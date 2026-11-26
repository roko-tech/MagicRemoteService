//! Builds a webOS package (.ipk) the way ares-package does: an `ar` archive holding
//! `debian-binary`, `control.tar.gz` and `data.tar.gz`, with the app under
//! `usr/palm/applications/<id>`, its service under `usr/palm/services/<id>.service` and
//! `usr/palm/packages/<id>/packageinfo.json` tying them together.

use flate2::write::GzEncoder;
use flate2::Compression;
use std::collections::BTreeSet;
use std::io::Write;

/// The package for `app_id`. `files` are `app/...` and `service/...` paths, as `tv::render`
/// returns them; `mtime` is the Unix time stamped on every entry.
pub fn build(app_id: &str, version: &str, files: &[(String, Vec<u8>)], mtime: u64) -> Vec<u8> {
    let service_id = format!("{app_id}.service");
    let packageinfo = serde_json::to_vec_pretty(&serde_json::json!({
        "id": app_id,
        "version": version,
        "app": app_id,
        "services": [service_id],
    }))
    .unwrap_or_default();
    let mut data: Vec<(String, &[u8])> = files
        .iter()
        .filter_map(|(path, bytes)| {
            let path = if let Some(rest) = path.strip_prefix("app/") {
                format!("usr/palm/applications/{app_id}/{rest}")
            } else {
                format!("usr/palm/services/{service_id}/{}", path.strip_prefix("service/")?)
            };
            Some((path, bytes.as_slice()))
        })
        .collect();
    data.push((format!("usr/palm/packages/{app_id}/packageinfo.json"), &packageinfo));
    data.sort_by(|a, b| a.0.cmp(&b.0));

    let installed_size: usize = data.iter().map(|(_, b)| b.len()).sum();
    let control = format!(
        "Package: {app_id}\nVersion: {version}\nSection: misc\nPriority: optional\nArchitecture: all\nInstalled-Size: {installed_size}\n\
         Maintainer: N/A <nobody@example.com>\nDescription: MagicRemoteService TV app\nwebOS-Package-Format-Version: 2\nwebOS-Packager-Version: x.y.x\n"
    );

    let mut out = b"!<arch>\n".to_vec();
    ar_member(&mut out, "debian-binary", b"2.0\n", mtime);
    ar_member(&mut out, "control.tar.gz", &gzip(&tar(&[("control".into(), control.as_bytes())], mtime)), mtime);
    ar_member(&mut out, "data.tar.gz", &gzip(&tar(&data, mtime)), mtime);
    out
}

fn ar_member(out: &mut Vec<u8>, name: &str, data: &[u8], mtime: u64) {
    out.extend_from_slice(format!("{name:<16}{mtime:<12}{:<6}{:<6}{:<8}{:<10}`\n", 0, 0, 100644, data.len()).as_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(b'\n');
    }
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::best());
    // Writing to memory can't fail
    let _ = gz.write_all(data);
    gz.finish().unwrap_or_default()
}

/// A ustar archive of `files`, with an entry for each folder above them.
fn tar(files: &[(String, &[u8])], mtime: u64) -> Vec<u8> {
    let dirs: BTreeSet<String> = files.iter().flat_map(|(path, _)| path.match_indices('/').map(|(i, _)| path[..=i].to_string())).collect();
    let mut out = Vec::new();
    for dir in &dirs {
        out.extend_from_slice(&tar_header(dir, 0, true, mtime));
    }
    for (path, data) in files {
        out.extend_from_slice(&tar_header(path, data.len() as u64, false, mtime));
        out.extend_from_slice(data);
        out.resize(out.len().next_multiple_of(512), 0);
    }
    out.resize(out.len() + 1024, 0);
    out
}

fn tar_header(path: &str, size: u64, dir: bool, mtime: u64) -> [u8; 512] {
    let mut h = [0u8; 512];
    // Names longer than 100 bytes go in the 155-byte prefix field, split at a slash
    let (prefix, name) = match path.len() {
        0..=100 => ("", path),
        _ => path
            .char_indices()
            .filter(|&(i, c)| c == '/' && i <= 155 && path.len() - i - 1 <= 100)
            .map(|(i, _)| (&path[..i], &path[i + 1..]))
            .next()
            .unwrap_or(("", path)),
    };
    let put = |h: &mut [u8; 512], at: usize, len: usize, v: &[u8]| h[at..at + v.len().min(len)].copy_from_slice(&v[..v.len().min(len)]);
    put(&mut h, 0, 100, name.as_bytes());
    put(&mut h, 100, 8, if dir { b"0000755\0" } else { b"0000644\0" });
    put(&mut h, 108, 8, b"0000000\0");
    put(&mut h, 116, 8, b"0000000\0");
    put(&mut h, 124, 12, format!("{size:011o}\0").as_bytes());
    put(&mut h, 136, 12, format!("{mtime:011o}\0").as_bytes());
    h[156] = if dir { b'5' } else { b'0' };
    put(&mut h, 257, 8, b"ustar\x0000");
    put(&mut h, 345, 155, prefix.as_bytes());
    // The checksum counts its own field as spaces
    h[148..156].fill(b' ');
    let sum: u32 = h.iter().map(|&b| b as u32).sum();
    put(&mut h, 148, 8, format!("{sum:06o}\0 ").as_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::io::Read;

    /// The members of an `ar` archive.
    fn ar_members(ipk: &[u8]) -> Vec<(String, Vec<u8>)> {
        assert_eq!(&ipk[..8], b"!<arch>\n");
        let mut out = Vec::new();
        let mut pos = 8;
        while pos < ipk.len() {
            let h = &ipk[pos..pos + 60];
            assert_eq!(&h[58..60], b"`\n");
            let name = String::from_utf8_lossy(&h[..16]).trim_end().to_string();
            let size: usize = String::from_utf8_lossy(&h[48..58]).trim().parse().unwrap();
            out.push((name, ipk[pos + 60..pos + 60 + size].to_vec()));
            pos += 60 + size + size % 2;
        }
        out
    }

    /// The entries of a tar.gz: (path, is folder, contents), checking each header's checksum.
    fn tar_entries(gz: &[u8]) -> Vec<(String, bool, Vec<u8>)> {
        let mut tar = Vec::new();
        GzDecoder::new(gz).read_to_end(&mut tar).unwrap();
        let mut out = Vec::new();
        let mut pos = 0;
        while tar[pos..pos + 512].iter().any(|&b| b != 0) {
            let h = &tar[pos..pos + 512];
            let field = |a: usize, n: usize| String::from_utf8_lossy(&h[a..a + n]).trim_end_matches(['\0', ' ']).to_string();
            let sum: u32 = h.iter().enumerate().map(|(i, &b)| if (148..156).contains(&i) { b' ' as u32 } else { b as u32 }).sum();
            assert_eq!(u32::from_str_radix(&field(148, 8), 8).unwrap(), sum);
            assert_eq!(&h[257..263], b"ustar\0");
            let size = usize::from_str_radix(&field(124, 12), 8).unwrap();
            let prefix = field(345, 155);
            let name = if prefix.is_empty() { field(0, 100) } else { format!("{prefix}/{}", field(0, 100)) };
            out.push((name, h[156] == b'5', tar[pos + 512..pos + 512 + size].to_vec()));
            pos += 512 + size.next_multiple_of(512);
        }
        out
    }

    #[test]
    fn packages_like_ares_package() {
        let files = vec![
            ("app/appinfo.json".to_string(), b"{\"id\":\"x\"}".to_vec()),
            ("app/webOSTVjs-1.2.12/webOSTV.js".to_string(), vec![7u8; 1000]),
            ("service/service.js".to_string(), b"odd".to_vec()),
        ];
        let ipk = build("com.example.app", "3.1.0", &files, 1_700_000_000);
        let members = ar_members(&ipk);
        let names: Vec<_> = members.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["debian-binary", "control.tar.gz", "data.tar.gz"]);
        assert_eq!(members[0].1, b"2.0\n");

        let control = tar_entries(&members[1].1);
        assert_eq!(control.len(), 1);
        let text = String::from_utf8(control[0].2.clone()).unwrap();
        assert!(text.starts_with("Package: com.example.app\nVersion: 3.1.0\n"), "{text}");
        let packageinfo_len = tar_entries(&members[2].1).iter().find(|e| e.0.ends_with("packageinfo.json")).unwrap().2.len();
        assert!(text.contains(&format!("Installed-Size: {}\n", 10 + 1000 + 3 + packageinfo_len)), "{text}");

        let data = tar_entries(&members[2].1);
        let paths: Vec<_> = data.iter().map(|(p, dir, _)| (p.as_str(), *dir)).collect();
        assert_eq!(
            paths,
            [
                ("usr/", true),
                ("usr/palm/", true),
                ("usr/palm/applications/", true),
                ("usr/palm/applications/com.example.app/", true),
                ("usr/palm/applications/com.example.app/webOSTVjs-1.2.12/", true),
                ("usr/palm/packages/", true),
                ("usr/palm/packages/com.example.app/", true),
                ("usr/palm/services/", true),
                ("usr/palm/services/com.example.app.service/", true),
                ("usr/palm/applications/com.example.app/appinfo.json", false),
                ("usr/palm/applications/com.example.app/webOSTVjs-1.2.12/webOSTV.js", false),
                ("usr/palm/packages/com.example.app/packageinfo.json", false),
                ("usr/palm/services/com.example.app.service/service.js", false),
            ]
        );
        assert_eq!(data[10].2, vec![7u8; 1000]);
        let info: serde_json::Value = serde_json::from_slice(&data[11].2).unwrap();
        assert_eq!(info, serde_json::json!({"id": "com.example.app", "version": "3.1.0", "app": "com.example.app", "services": ["com.example.app.service"]}));
    }

    #[test]
    fn long_paths_use_the_prefix_field() {
        let long = format!("usr/palm/applications/{}/file.js", "a".repeat(90));
        let entries = tar_entries(&gzip(&tar(&[(long.clone(), b"x")], 0)));
        assert_eq!(entries.last().unwrap().0, long);
    }
}
