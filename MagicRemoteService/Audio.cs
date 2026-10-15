namespace MagicRemoteService {
	// The Windows default playback device, read through the Core Audio COM
	// interfaces (IMMDeviceEnumerator, IMMDevice, IPropertyStore).
	internal static class Audio {
		[System.Runtime.InteropServices.ComImport]
		[System.Runtime.InteropServices.Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")]
		private class MMDeviceEnumerator {
		}
		[System.Runtime.InteropServices.ComImport]
		[System.Runtime.InteropServices.Guid("A95664D2-9614-4F35-A746-DE8DB63617E6")]
		[System.Runtime.InteropServices.InterfaceType(System.Runtime.InteropServices.ComInterfaceType.InterfaceIsIUnknown)]
		private interface IMMDeviceEnumerator {
			[System.Runtime.InteropServices.PreserveSig]
			int EnumAudioEndpoints(int dataFlow, uint dwStateMask, out System.IntPtr ppDevices);
			[System.Runtime.InteropServices.PreserveSig]
			int GetDefaultAudioEndpoint(int dataFlow, int role, out IMMDevice ppEndpoint);
		}
		[System.Runtime.InteropServices.ComImport]
		[System.Runtime.InteropServices.Guid("D666063F-1587-4E43-81F1-B948E807363F")]
		[System.Runtime.InteropServices.InterfaceType(System.Runtime.InteropServices.ComInterfaceType.InterfaceIsIUnknown)]
		private interface IMMDevice {
			[System.Runtime.InteropServices.PreserveSig]
			int Activate(ref System.Guid iid, uint dwClsCtx, System.IntPtr pActivationParams, out System.IntPtr ppInterface);
			[System.Runtime.InteropServices.PreserveSig]
			int OpenPropertyStore(uint stgmAccess, out IPropertyStore ppProperties);
		}
		[System.Runtime.InteropServices.ComImport]
		[System.Runtime.InteropServices.Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99")]
		[System.Runtime.InteropServices.InterfaceType(System.Runtime.InteropServices.ComInterfaceType.InterfaceIsIUnknown)]
		private interface IPropertyStore {
			[System.Runtime.InteropServices.PreserveSig]
			int GetCount(out uint cProps);
			[System.Runtime.InteropServices.PreserveSig]
			int GetAt(uint iProp, out PropertyKey pkey);
			[System.Runtime.InteropServices.PreserveSig]
			int GetValue(ref PropertyKey key, out PropVariant pv);
		}
		[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
		private struct PropertyKey {
			public System.Guid fmtid;
			public uint pid;
		}
		[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
		private struct PropVariant {
			public ushort vt;
			public ushort wReserved1;
			public ushort wReserved2;
			public ushort wReserved3;
			public System.IntPtr p; // first pointer-size bytes of the value union
			public System.IntPtr p2;
		}
		[System.Runtime.InteropServices.DllImport("ole32.dll")]
		private static extern int PropVariantClear(ref PropVariant pvar);

		private const int E_RENDER = 0;
		private const int E_CONSOLE = 0;
		private const uint STGM_READ = 0;
		private const ushort VT_UI4 = 19;
		private const uint UNKNOWN_FORM_FACTOR = 10;
		private static readonly System.Guid PKEY_AUDIO_ENDPOINT_FORM_FACTOR = new System.Guid("1DA5D803-D492-4EDD-8C23-E0C0FFEE7F0E");

		// True when Windows plays on a device other than the TV, such as
		// headphones: any endpoint form factor but unknown digital passthrough (7),
		// S/PDIF (8), and HDMI or DisplayPort (9). False when it plays on the TV,
		// has no playback device, or the device cannot be read.
		public static bool DefaultIsLocal() {
			IMMDeviceEnumerator mmde = null;
			IMMDevice mmd = null;
			IPropertyStore ps = null;
			try {
				mmde = (IMMDeviceEnumerator)new MMDeviceEnumerator();
				if(mmde.GetDefaultAudioEndpoint(E_RENDER, E_CONSOLE, out mmd) < 0 || mmd.OpenPropertyStore(STGM_READ, out ps) < 0) {
					return false;
				}
				PropertyKey pk = new PropertyKey {
					fmtid = PKEY_AUDIO_ENDPOINT_FORM_FACTOR,
					pid = 0
				};
				if(ps.GetValue(ref pk, out PropVariant pv) < 0) {
					return false;
				}
				uint uiFormFactor = pv.vt == VT_UI4 ? (uint)(pv.p.ToInt64() & 0xFFFFFFFF) : UNKNOWN_FORM_FACTOR;
				PropVariantClear(ref pv);
				return uiFormFactor != 7 && uiFormFactor != 8 && uiFormFactor != 9;
			} catch(System.Runtime.InteropServices.COMException) {
				return false;
			} finally {
				foreach(object o in new object[] { ps, mmd, mmde }) {
					if(o != null) {
						System.Runtime.InteropServices.Marshal.ReleaseComObject(o);
					}
				}
			}
		}
	}
}
