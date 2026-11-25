//! Where Windows plays sound: the default render endpoint's form factor.

use windows::core::GUID;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ};
use windows::Win32::System::Variant::VT_UI4;

const PKEY_AUDIOENDPOINT_FORMFACTOR: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0x1da5d803_d492_4edd_8c23_e0c0ffee7f0e), pid: 0 };

/// Endpoint form factors that carry sound to the TV: unknown digital passthrough,
/// S/PDIF (a soundbar on the TV's optical out or eARC) and HDMI or DisplayPort.
fn plays_on_tv(form_factor: u32) -> bool {
    matches!(form_factor, 7..=9)
}

fn form_factor() -> windows::core::Result<Option<u32>> {
    unsafe {
        // Each connection thread initializes COM once; repeated calls are harmless.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let store = device.OpenPropertyStore(STGM_READ)?;
        let value = store.GetValue(&PKEY_AUDIOENDPOINT_FORMFACTOR)?;
        let raw = &value.Anonymous.Anonymous;
        // A UI4 holds nothing to free
        Ok((raw.vt == VT_UI4).then(|| raw.Anonymous.ulVal))
    }
}

/// True when Windows plays on something other than the TV, such as headphones. False when it
/// plays on the TV, has no playback device, or the device can't be read.
pub fn default_is_local() -> bool {
    match form_factor() {
        Ok(Some(ff)) => !plays_on_tv(ff),
        // No form factor reads as "unknown", which is not the TV
        Ok(None) => true,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "reads this PC's default playback device"]
    fn reads_the_default_endpoint() {
        let ff = super::form_factor();
        println!("form factor {ff:?}, plays locally: {}", super::default_is_local());
        assert!(ff.is_ok());
    }

    #[test]
    fn hdmi_spdif_and_passthrough_mean_the_tv() {
        for (ff, tv) in [(1, false), (3, false), (7, true), (8, true), (9, true), (10, false)] {
            assert_eq!(super::plays_on_tv(ff), tv, "form factor {ff}");
        }
    }
}
