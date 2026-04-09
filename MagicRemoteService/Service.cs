
namespace MagicRemoteService {
	public enum ServiceType {
		Server,
		Client,
		Both
	}
	public enum WebSocketOpCode : byte {
		Continuation = 0x0,
		Text = 0x1,
		Binary = 0x2,
		ConnectionClose = 0x8,
		Ping = 0x9,
		Pong = 0xA
	}
	public enum MessageType : byte {
		PositionRelative = 0x00,
		PositionAbsolute = 0x01,
		Wheel = 0x02,
		Visible = 0x03,
		Key = 0x04,
		Unicode = 0x05,
		Shutdown = 0x06
	}
	public partial class Service : System.ServiceProcess.ServiceBase {
		private int iPort;
		private volatile bool bInactivity;
		private int iTimeoutInactivity;
		private volatile bool bVideoInput;
		private int iTimeoutVideoInput;
		private System.Threading.Thread thrSsdp;
		private System.Threading.Thread thrHttpSettings;
		private static readonly string SSDP_MULTICAST = "239.255.255.250";
		private static readonly int SSDP_PORT = 1900;
		private static readonly string SSDP_USN = "urn:magicremoteservice:service:remote:1";
		private readonly System.Collections.Generic.Dictionary<ushort, Bind[]> dBind = new System.Collections.Generic.Dictionary<ushort, Bind[]>() {
			{ 0x0001, null },
			{ 0x0002, null },
			{ 0x0008, null },
			{ 0x000D, null },
			{ 0x0021, null },
			{ 0x0022, null },
			{ 0x0025, null },
			{ 0x0026, null },
			{ 0x0027, null },
			{ 0x0028, null },
			{ 0x0030, null },
			{ 0x0031, null },
			{ 0x0032, null },
			{ 0x0033, null },
			{ 0x0034, null },
			{ 0x0035, null },
			{ 0x0036, null },
			{ 0x0037, null },
			{ 0x0038, null },
			{ 0x0039, null },
			{ 0x0193, null },
			{ 0x0194, null },
			{ 0x0195, null },
			{ 0x0196, null },
			{ 0x01CD, null },
			{ 0x019F, null },
			{ 0x0013, null },
			{ 0x01A1, null },
			{ 0x019C, null },
			{ 0x019D, null }
		};

		private static readonly System.Diagnostics.EventLog elEventLog = new System.Diagnostics.EventLog("Application", ".", "MagicRemoteService");

		private System.Threading.Thread thrServer;
		private ServiceType stType;

		private static readonly System.Threading.ManualResetEvent mreStop = new System.Threading.ManualResetEvent(true);
		private static int iConnectedClients = 0;
		private static string strConnectedClientIp = "";
		private static readonly System.Threading.AutoResetEvent areSessionChanged = new System.Threading.AutoResetEvent(false);
		private static System.Threading.EventWaitHandle ewhServerStarted;
		private static System.Threading.EventWaitHandle ewhClientStarted;
		private static System.Threading.EventWaitHandle ewhSessionChanged;
		private static System.Threading.EventWaitHandle ewhClientConnecting;
		private static System.Threading.EventWaitHandle ewhServerMessage;
		private static System.Threading.EventWaitHandle ewhServerDisconnecting;

		private static readonly byte[] tabClose = { (0b1000 << 4) | (0x8 << 0), 0x00 };
		private static readonly byte[] tabPing = { (0b1000 << 4) | (0x9 << 0), 0x00 };
		private static readonly byte[] tabPingUserInput = { (0b1000 << 4) | (0x9 << 0), 0x01, 0x01 };
		static Service() {
			if(!System.Diagnostics.EventLog.SourceExists("MagicRemoteService")) {
				System.Diagnostics.EventLog.CreateEventSource("MagicRemoteService", "Application");
			}
		}
		public Service() {
			this.InitializeComponent();
		}
		public void ServiceStart() {
			Service.ewhServerStarted = new System.Threading.EventWaitHandle(false, System.Threading.EventResetMode.ManualReset, @"Global\{FFB31601-E362-48A5-B9A2-5DF29A3B06C1}", out _, Program.ewhsAll);
			Service.ewhClientStarted = new System.Threading.EventWaitHandle(false, System.Threading.EventResetMode.ManualReset, @"Global\{9878BC83-46A0-412B-86B6-10F1C43FC0D9}", out _, Program.ewhsAll);
			Service.ewhSessionChanged = new System.Threading.EventWaitHandle(false, System.Threading.EventResetMode.AutoReset, @"Global\{996C2D37-8FAC-4C89-8A00-CE30CBE66B87}", out _, Program.ewhsAll);
			Service.ewhClientConnecting = new System.Threading.EventWaitHandle(false, System.Threading.EventResetMode.AutoReset, @"Global\{06031D31-621A-4288-850E-8FEE0ED3F054}", out _, Program.ewhsAll);
			Service.ewhServerMessage = new System.Threading.EventWaitHandle(false, System.Threading.EventResetMode.AutoReset, @"Global\{78968501-8AE0-424D-B82E-EA0A0BEA3414}", out _, Program.ewhsAll);
			Service.ewhServerDisconnecting = new System.Threading.EventWaitHandle(false, System.Threading.EventResetMode.AutoReset, @"Global\{C45127E0-B626-46D0-8610-C5DE20E4F790}", out _, Program.ewhsAll);

			if(!System.Environment.UserInteractive) {
				this.stType = ServiceType.Server;
			} else if(!Service.ewhServerStarted.WaitOne(System.TimeSpan.Zero)) {
				this.stType = ServiceType.Both;
			} else {
				this.stType = ServiceType.Client;
			}

			WinApi.ServiceStatus ssServiceStatus = new WinApi.ServiceStatus();
			switch(this.stType) {
				case ServiceType.Server:
					Service.Log("Service server start");
					break;
				case ServiceType.Both:
					Service.Log("Service both start");
					break;
				case ServiceType.Client:
					Service.Log("Service client start");
					break;
			}
			switch(this.stType) {
				case ServiceType.Server:
					ssServiceStatus.dwCurrentState = WinApi.ServiceCurrentState.SERVICE_START_PENDING;
					ssServiceStatus.dwWaitHint = 100000;
					if(!WinApi.Advapi32.SetServiceStatus(this.ServiceHandle, ref ssServiceStatus)) {
					Service.Warn("Failed to set service status to START_PENDING");
				}
					break;
				case ServiceType.Both:
				case ServiceType.Client:
					break;
			}
			Microsoft.Win32.RegistryKey rkMagicRemoteService = (MagicRemoteService.Program.bElevated ? Microsoft.Win32.Registry.LocalMachine : Microsoft.Win32.Registry.CurrentUser).OpenSubKey(@"Software\MagicRemoteService");
			switch(this.stType) {
				case ServiceType.Server:
				case ServiceType.Both:
					if(rkMagicRemoteService == null) {
						System.Threading.Interlocked.Exchange(ref this.iPort, 41230);
					} else {
						System.Threading.Interlocked.Exchange(ref this.iPort, (int)rkMagicRemoteService.GetValue("Port", 41230));
					}
					break;
				case ServiceType.Client:
					break;
			}
			switch(this.stType) {
				case ServiceType.Server:
					break;
				case ServiceType.Both:
				case ServiceType.Client:
					if(rkMagicRemoteService == null) {
						this.bInactivity = true;
						System.Threading.Interlocked.Exchange(ref this.iTimeoutInactivity, 7200000);
						this.bVideoInput = true;
						System.Threading.Interlocked.Exchange(ref this.iTimeoutVideoInput, 900000);
					} else {
						this.bInactivity = (int)rkMagicRemoteService.GetValue("Inactivity", 1) != 0;
						System.Threading.Interlocked.Exchange(ref this.iTimeoutInactivity, (int)rkMagicRemoteService.GetValue("TimeoutInactivity", 7200000));
						this.bVideoInput = (int)rkMagicRemoteService.GetValue("VideoInput", 1) != 0;
						System.Threading.Interlocked.Exchange(ref this.iTimeoutVideoInput, (int)rkMagicRemoteService.GetValue("TimeoutVideoInput", 900000));
					}
					// Try loading from bindings.json first (next to exe)
					string strBindingsPath = System.IO.Path.Combine(System.IO.Path.GetDirectoryName(System.Reflection.Assembly.GetExecutingAssembly().Location), "bindings.json");
					if(System.IO.File.Exists(strBindingsPath)) {
						this.LoadBindingsFromJson(strBindingsPath);
					} else {
					Microsoft.Win32.RegistryKey rkMagicRemoteServiceRemoteBind = (MagicRemoteService.Program.bElevated ? Microsoft.Win32.Registry.LocalMachine : Microsoft.Win32.Registry.CurrentUser).OpenSubKey(@"Software\MagicRemoteService\Remote\Bind");
					if(rkMagicRemoteServiceRemoteBind == null) {
						this.dBind[0x0001] = new Bind[] { new BindMouse(BindMouseValue.Left) };
						this.dBind[0x0002] = new Bind[] { new BindMouse(BindMouseValue.Right) };
						this.dBind[0x0008] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.Back, 0x0E, false) };
						this.dBind[0x000D] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.Enter, 0x1C, false) };
						this.dBind[0x0021] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.ControlKey, 0x1D, false), new BindKeyboard((byte)System.Windows.Forms.Keys.C, 0x2E, false) };
						this.dBind[0x0022] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.ControlKey, 0x1D, false), new BindKeyboard((byte)System.Windows.Forms.Keys.V, 0x2F, false) };
						this.dBind[0x0025] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.Left, 0x4B, true) };
						this.dBind[0x0026] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.Up, 0x48, true) };
						this.dBind[0x0027] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.Right, 0x4D, true) };
						this.dBind[0x0028] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.Down, 0x50, true) };
						this.dBind[0x0030] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.NumPad0, 0x52, false) };
						this.dBind[0x0031] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.NumPad1, 0x4F, false) };
						this.dBind[0x0032] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.NumPad2, 0x50, false) };
						this.dBind[0x0033] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.NumPad3, 0x51, false) };
						this.dBind[0x0034] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.NumPad4, 0x4B, false) };
						this.dBind[0x0035] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.NumPad5, 0x4C, false) };
						this.dBind[0x0036] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.NumPad6, 0x4D, false) };
						this.dBind[0x0037] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.NumPad7, 0x47, false) };
						this.dBind[0x0038] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.NumPad8, 0x48, false) };
						this.dBind[0x0039] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.NumPad9, 0x49, false) };
						this.dBind[0x0193] = new Bind[] { new BindAction(BindActionValue.Shutdown) };
						this.dBind[0x0194] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.LWin, 0x5B, true) };
						this.dBind[0x0195] = new Bind[] { new BindMouse(BindMouseValue.Right) };
						this.dBind[0x0196] = new Bind[] { new BindAction(BindActionValue.Keyboard) };
						this.dBind[0x01CD] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.Escape, 0x01, false) };
						this.dBind[0x019F] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.Play, 0x00, false) };
						this.dBind[0x0013] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.Pause, 0x00, false) };
						this.dBind[0x01A1] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.MediaNextTrack, 0x00, false) };
						this.dBind[0x019C] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.MediaPreviousTrack, 0x00, false) };
						this.dBind[0x019D] = new Bind[] { new BindKeyboard((byte)System.Windows.Forms.Keys.MediaStop, 0x00, false) };
					} else {
						foreach(string sKey in rkMagicRemoteServiceRemoteBind.GetSubKeyNames()) {
							System.Collections.Generic.List<Bind> liBind = new System.Collections.Generic.List<Bind>();
							Microsoft.Win32.RegistryKey rkMagicRemoteServiceRemoteBindKey = rkMagicRemoteServiceRemoteBind.OpenSubKey(sKey);
							foreach(string sBind in rkMagicRemoteServiceRemoteBindKey.GetSubKeyNames()) {
								Microsoft.Win32.RegistryKey rkMagicRemoteServiceRemoteBindBind = rkMagicRemoteServiceRemoteBindKey.OpenSubKey(sBind);
								switch((int)rkMagicRemoteServiceRemoteBindBind.GetValue("Kind")) {
									case 0x00:
										liBind.Add(new BindMouse((BindMouseValue)(int)rkMagicRemoteServiceRemoteBindBind.GetValue("Value", 0x0000)));
										break;
									case 0x01:
										liBind.Add(new BindKeyboard((byte)(int)rkMagicRemoteServiceRemoteBindBind.GetValue("VirtualKey", 0x00), (byte)(int)rkMagicRemoteServiceRemoteBindBind.GetValue("ScanCode", 0x00), (int)rkMagicRemoteServiceRemoteBindBind.GetValue("Extended", 0x00) == 0x01));
										break;
									case 0x02:
										liBind.Add(new BindAction((BindActionValue)(int)rkMagicRemoteServiceRemoteBindBind.GetValue("Value", 0x00)));
										break;
									case 0x03:
										liBind.Add(new BindCommand((string)rkMagicRemoteServiceRemoteBindBind.GetValue("Command")));
										break;
								}
							}
							this.dBind[ushort.Parse(sKey.Substring(2), System.Globalization.NumberStyles.HexNumber)] = liBind.ToArray();
						}
					}
					}
					break;
			}
			rkMagicRemoteService?.Close();
			switch(this.stType) {
				case ServiceType.Server:
					Service.ewhServerStarted.Set();
					break;
				case ServiceType.Both:
					break;
				case ServiceType.Client:
					Service.ewhClientStarted.Set();
					break;
			}
			Service.mreStop.Reset();
			this.thrServer = new System.Threading.Thread(delegate () {
				this.ThreadServer();
			});
			this.thrServer.Start();
			switch(this.stType) {
				case ServiceType.Server:
				case ServiceType.Both:
					this.thrSsdp = new System.Threading.Thread(delegate () {
						this.ThreadSsdp();
					});
					this.thrSsdp.IsBackground = true;
					this.thrSsdp.Start();
					Service.Log("SSDP discovery responder started on port " + SSDP_PORT);
					this.thrHttpSettings = new System.Threading.Thread(delegate () {
						this.ThreadHttpSettings();
					});
					this.thrHttpSettings.IsBackground = true;
					this.thrHttpSettings.Start();
					Service.Log("Web settings UI started on http://localhost:" + (this.iPort + 1));
					break;
				case ServiceType.Client:
					break;
			}
			switch(this.stType) {
				case ServiceType.Server:
					Service.Log("Service server started");
					break;
				case ServiceType.Both:
					Service.Log("Service both started");
					break;
				case ServiceType.Client:
					Service.Log("Service client started");
					break;
			}
			switch(this.stType) {
				case ServiceType.Server:
					ssServiceStatus.dwCurrentState = WinApi.ServiceCurrentState.SERVICE_RUNNING;
					WinApi.Advapi32.SetServiceStatus(this.ServiceHandle, ref ssServiceStatus);
					break;
				case ServiceType.Both:
				case ServiceType.Client:
					break;
			}
		}
		public void ServiceStop() {

			WinApi.ServiceStatus ssServiceStatus = new WinApi.ServiceStatus();
			switch(this.stType) {
				case ServiceType.Server:
					Service.Log("Service server stop");
					break;
				case ServiceType.Both:
					Service.Log("Service both stop");
					break;
				case ServiceType.Client:
					Service.Log("Service client stop");
					break;
			}
			switch(this.stType) {
				case ServiceType.Server:
					ssServiceStatus.dwCurrentState = WinApi.ServiceCurrentState.SERVICE_STOP_PENDING;
					ssServiceStatus.dwWaitHint = 100000;
					WinApi.Advapi32.SetServiceStatus(this.ServiceHandle, ref ssServiceStatus);
					break;
				case ServiceType.Both:
				case ServiceType.Client:
					break;
			}
			switch(this.stType) {
				case ServiceType.Server:
					Service.ewhServerStarted.Reset();
					break;
				case ServiceType.Both:
					break;
				case ServiceType.Client:
					Service.ewhClientStarted.Reset();
					break;
			}
			Service.mreStop.Set();
			this.thrServer.Join(System.TimeSpan.FromSeconds(10));
			this.thrServer = null;
			switch(this.stType) {
				case ServiceType.Server:
					Service.Log("Service server stoped");
					break;
				case ServiceType.Both:
					Service.Log("Service both stoped");
					break;
				case ServiceType.Client:
					Service.Log("Service client stoped");
					break;
			}
			switch(this.stType) {
				case ServiceType.Server:
					ssServiceStatus.dwCurrentState = WinApi.ServiceCurrentState.SERVICE_STOPPED;
					WinApi.Advapi32.SetServiceStatus(this.ServiceHandle, ref ssServiceStatus);
					break;
				case ServiceType.Both:
				case ServiceType.Client:
					break;
			}
			switch(this.stType) {
				case ServiceType.Server:
					break;
				case ServiceType.Both:
				case ServiceType.Client:
					break;
			}

			Service.ewhServerStarted.Close();
			Service.ewhServerStarted.Dispose();
			Service.ewhClientStarted.Close();
			Service.ewhClientStarted.Dispose();
			Service.ewhSessionChanged.Close();
			Service.ewhSessionChanged.Dispose();
			Service.ewhClientConnecting.Close();
			Service.ewhClientConnecting.Dispose();
			Service.ewhServerMessage.Close();
			Service.ewhServerMessage.Dispose();
			Service.ewhServerDisconnecting.Close();
			Service.ewhServerDisconnecting.Dispose();
		}
		protected override void OnStart(string[] args) {
			this.ServiceStart();
		}
		protected override void OnStop() {
			this.ServiceStop();
		}
		protected override void OnSessionChange(System.ServiceProcess.SessionChangeDescription scd) {
			switch(scd.Reason) {
				case System.ServiceProcess.SessionChangeReason.ConsoleConnect:
					Service.areSessionChanged.Set();
					break;
				default:
					break;
			}
		}
		public static void Log(string sLog) {
			Service.elEventLog.WriteEntry(sLog, System.Diagnostics.EventLogEntryType.Information);
		}
		public static void LogIfDebug(string sLog) {
#if DEBUG
			Service.Log(sLog);
#endif
		}
		public static void Warn(string sWarn) {
			Service.elEventLog.WriteEntry(sWarn, System.Diagnostics.EventLogEntryType.Warning);
		}
		public static void Error(string sError) {
			Service.elEventLog.WriteEntry(sError, System.Diagnostics.EventLogEntryType.Error);
		}
		private static void SerializeSocketInfo(System.IO.Stream stream, System.Net.Sockets.SocketInformation si) {
			byte[] protocolInfo = si.ProtocolInformation;
			byte[] lenBytes = System.BitConverter.GetBytes(protocolInfo.Length);
			stream.Write(lenBytes, 0, lenBytes.Length);
			stream.Write(protocolInfo, 0, protocolInfo.Length);
			byte[] optBytes = System.BitConverter.GetBytes((int)si.Options);
			stream.Write(optBytes, 0, optBytes.Length);
			stream.Flush();
		}
		private static System.Net.Sockets.SocketInformation DeserializeSocketInfo(System.IO.Stream stream) {
			byte[] lenBytes = new byte[4];
			int bytesRead = stream.Read(lenBytes, 0, 4);
			if(bytesRead < 4) throw new System.IO.IOException("Failed to read socket info length");
			int len = System.BitConverter.ToInt32(lenBytes, 0);
			if(len < 0 || len > 4096) throw new System.IO.IOException("Invalid socket info length: " + len);
			byte[] protocolInfo = new byte[len];
			int totalRead = 0;
			while(totalRead < len) {
				bytesRead = stream.Read(protocolInfo, totalRead, len - totalRead);
				if(bytesRead <= 0) throw new System.IO.IOException("Failed to read socket info data");
				totalRead += bytesRead;
			}
			byte[] optBytes = new byte[4];
			bytesRead = stream.Read(optBytes, 0, 4);
			if(bytesRead < 4) throw new System.IO.IOException("Failed to read socket info options");
			return new System.Net.Sockets.SocketInformation {
				ProtocolInformation = protocolInfo,
				Options = (System.Net.Sockets.SocketInformationOptions)System.BitConverter.ToInt32(optBytes, 0)
			};
		}
		private static bool SetThreadInputDesktop() {
			System.IntPtr hInputDesktop = WinApi.User32.OpenInputDesktop(0, true, 0x10000000);
			if(System.IntPtr.Zero == hInputDesktop) {
				return false;
			} else if(!WinApi.User32.SetThreadDesktop(hInputDesktop)) {
				WinApi.User32.CloseDesktop(hInputDesktop);
				return false;
			} else {
				WinApi.User32.CloseDesktop(hInputDesktop);
				return true;
			}
		}
		private static uint SendInputAdmin(WinApi.Input[] pInputs) {
			uint uiInput = WinApi.User32.SendInput((uint)pInputs.Length, pInputs, System.Runtime.InteropServices.Marshal.SizeOf(typeof(WinApi.Input)));
			if(0 == uiInput && SetThreadInputDesktop()) {
				return WinApi.User32.SendInput((uint)pInputs.Length, pInputs, System.Runtime.InteropServices.Marshal.SizeOf(typeof(WinApi.Input)));
			} else {
				return uiInput;
			}
		}
		private static uint OpenUserInteractiveProcess(string strApplication, string strArgument) {
			uint uiSessionId = WinApi.Kernel32.WTSGetActiveConsoleSessionId();
			if(uiSessionId == 0xFFFFFFFF) {
				return 0;
			} else {
				System.Diagnostics.Process[] arrWinlogon = System.Diagnostics.Process.GetProcessesByName("winlogon");
				System.Diagnostics.Process pWinlogon = System.Array.Find<System.Diagnostics.Process>(arrWinlogon, delegate (System.Diagnostics.Process p) {
					return (uint)p.SessionId == uiSessionId;
				});
				foreach(System.Diagnostics.Process pWl in arrWinlogon) {
					if(pWl != pWinlogon) pWl.Dispose();
				};
				if(pWinlogon == null) {
					throw new System.Exception("Unable to get winlogon process");
				}

				System.IntPtr hProcessToken;
				if(!WinApi.Advapi32.OpenProcessToken(pWinlogon.Handle, 0x0002, out hProcessToken)) {
					throw new System.ComponentModel.Win32Exception(System.Runtime.InteropServices.Marshal.GetLastWin32Error());
				}

				System.IntPtr hProcessTokenDupplicate;
				WinApi.SecurityAttributes sa = new WinApi.SecurityAttributes();
				sa.Length = System.Runtime.InteropServices.Marshal.SizeOf(sa);
				if(!WinApi.Advapi32.DuplicateTokenEx(hProcessToken, WinApi.Advapi32.MAXIMUM_ALLOWED, ref sa, WinApi.SecurityImpersonationLevel.SecurityImpersonation, WinApi.TokenType.TokenPrimary, out hProcessTokenDupplicate)) {
					WinApi.Kernel32.CloseHandle(hProcessToken);
					throw new System.ComponentModel.Win32Exception(System.Runtime.InteropServices.Marshal.GetLastWin32Error());
				}
				WinApi.Kernel32.CloseHandle(hProcessToken);

				System.IntPtr lpEnvironmentBlock;
				System.IntPtr hUserToken;
				if(!WinApi.Wtsapi32.WTSQueryUserToken(uiSessionId, out hUserToken)) {
					lpEnvironmentBlock = System.IntPtr.Zero;
					//throw new System.ComponentModel.Win32Exception(System.Runtime.InteropServices.Marshal.GetLastWin32Error());
				} else {
					if(!WinApi.Userenv.CreateEnvironmentBlock(out lpEnvironmentBlock, hUserToken, true)) {
						WinApi.Kernel32.CloseHandle(hUserToken);
						throw new System.ComponentModel.Win32Exception(System.Runtime.InteropServices.Marshal.GetLastWin32Error());
					}
					WinApi.Kernel32.CloseHandle(hUserToken);
				}

				WinApi.StartupInfo si = new WinApi.StartupInfo();
				WinApi.ProcessInformation piProcess;
				si.cb = System.Runtime.InteropServices.Marshal.SizeOf(si);
				si.lpDesktop = @"winsta0\default";
				if(!WinApi.Advapi32.CreateProcessAsUser(hProcessTokenDupplicate, strApplication, strArgument, ref sa, ref sa, false, 0x00000400, lpEnvironmentBlock, System.IO.Path.GetDirectoryName(strApplication), ref si, out piProcess)) {
					WinApi.Kernel32.CloseHandle(hProcessTokenDupplicate);
					if(lpEnvironmentBlock != System.IntPtr.Zero) {
						WinApi.Userenv.DestroyEnvironmentBlock(lpEnvironmentBlock);
					}
					throw new System.ComponentModel.Win32Exception(System.Runtime.InteropServices.Marshal.GetLastWin32Error());
				}

				WinApi.Kernel32.CloseHandle(hProcessTokenDupplicate);
				if(lpEnvironmentBlock != System.IntPtr.Zero) {
					WinApi.Userenv.DestroyEnvironmentBlock(lpEnvironmentBlock);
				}

				return piProcess.dwProcessId;
			}
		}
		private void LoadBindingsFromJson(string strPath) {
			try {
				string strJson = System.IO.File.ReadAllText(strPath);
				using(System.Text.Json.JsonDocument doc = System.Text.Json.JsonDocument.Parse(strJson)) {
					System.Text.Json.JsonElement elBindings = doc.RootElement.GetProperty("bindings");
					foreach(System.Text.Json.JsonProperty prop in elBindings.EnumerateObject()) {
						ushort usKey = ushort.Parse(prop.Name.Replace("0x", ""), System.Globalization.NumberStyles.HexNumber);
						System.Collections.Generic.List<Bind> liBind = new System.Collections.Generic.List<Bind>();
						foreach(System.Text.Json.JsonElement el in prop.Value.EnumerateArray()) {
							string strType = el.GetProperty("type").GetString();
							switch(strType) {
								case "mouse":
									string strMouse = el.GetProperty("value").GetString();
									BindMouseValue bmv = strMouse == "left" ? BindMouseValue.Left : strMouse == "right" ? BindMouseValue.Right : BindMouseValue.Middle;
									liBind.Add(new BindMouse(bmv));
									break;
								case "keyboard":
									liBind.Add(new BindKeyboard((byte)el.GetProperty("virtualKey").GetInt32(), (byte)el.GetProperty("scanCode").GetInt32(), el.GetProperty("extended").GetBoolean()));
									break;
								case "action":
									string strAction = el.GetProperty("value").GetString();
									liBind.Add(new BindAction(strAction == "shutdown" ? BindActionValue.Shutdown : BindActionValue.Keyboard));
									break;
								case "command":
									liBind.Add(new BindCommand(el.GetProperty("command").GetString()));
									break;
							}
						}
						this.dBind[usKey] = liBind.ToArray();
					}
				}
				Service.Log("Loaded key bindings from " + strPath);
			} catch(System.Exception ex) {
				Service.Warn("Failed to load bindings.json: " + ex.Message);
			}
		}
		private void ThreadHttpSettings() {
			int iHttpPort = this.iPort + 1;
			System.Net.HttpListener hlSettings = null;
			try {
				hlSettings = new System.Net.HttpListener();
				hlSettings.Prefixes.Add("http://localhost:" + iHttpPort + "/");
				hlSettings.Prefixes.Add("http://127.0.0.1:" + iHttpPort + "/");
				hlSettings.Start();
				Service.Log("HTTP listener started on port " + iHttpPort);
				while(hlSettings.IsListening) {
					System.Net.HttpListenerContext ctx = null;
					try {
						ctx = hlSettings.GetContext();
					} catch(System.Net.HttpListenerException) {
						break;
					}
					if(ctx != null) {
						try {
							string strReqPath = ctx.Request.Url.AbsolutePath;
							if(strReqPath == "/api/settings" && ctx.Request.HttpMethod == "GET") {
								string strBindingsFile = System.IO.Path.Combine(System.IO.Path.GetDirectoryName(System.Reflection.Assembly.GetExecutingAssembly().Location), "bindings.json");
								string strBindings = System.IO.File.Exists(strBindingsFile) ? System.IO.File.ReadAllText(strBindingsFile) : "{}";
								string strJson = "{\"port\":" + this.iPort + ",\"inactivity\":" + (this.bInactivity ? "true" : "false") + ",\"timeoutInactivity\":" + this.iTimeoutInactivity + ",\"videoInput\":" + (this.bVideoInput ? "true" : "false") + ",\"timeoutVideoInput\":" + this.iTimeoutVideoInput + ",\"connectedClients\":" + Service.iConnectedClients + ",\"clientIp\":\"" + Service.strConnectedClientIp + "\",\"bindings\":" + strBindings + "}";
								byte[] buf = System.Text.Encoding.UTF8.GetBytes(strJson);
								ctx.Response.ContentType = "application/json";
								ctx.Response.ContentLength64 = buf.Length;
								ctx.Response.OutputStream.Write(buf, 0, buf.Length);
							} else if(strReqPath == "/api/bindings" && ctx.Request.HttpMethod == "POST") {
								using(System.IO.StreamReader sr = new System.IO.StreamReader(ctx.Request.InputStream)) {
									string strBody = sr.ReadToEnd();
									System.Text.Json.JsonDocument.Parse(strBody).Dispose();
									string strBindingsFile = System.IO.Path.Combine(System.IO.Path.GetDirectoryName(System.Reflection.Assembly.GetExecutingAssembly().Location), "bindings.json");
									System.IO.File.WriteAllText(strBindingsFile, strBody);
									byte[] buf = System.Text.Encoding.UTF8.GetBytes("{\"saved\":true}");
									ctx.Response.ContentType = "application/json";
									ctx.Response.ContentLength64 = buf.Length;
									ctx.Response.OutputStream.Write(buf, 0, buf.Length);
									Service.Log("Key bindings saved via web UI");
								}
							} else if(strReqPath == "/api/restart" && ctx.Request.HttpMethod == "POST") {
								byte[] buf = System.Text.Encoding.UTF8.GetBytes("{\"restarting\":true}");
								ctx.Response.ContentType = "application/json";
								ctx.Response.ContentLength64 = buf.Length;
								ctx.Response.OutputStream.Write(buf, 0, buf.Length);
								ctx.Response.Close();
								Service.Log("Service restart requested via web UI");
								System.Threading.Tasks.Task.Run(delegate () {
									System.Diagnostics.Process pRestart = new System.Diagnostics.Process();
									pRestart.StartInfo.FileName = "cmd";
									pRestart.StartInfo.Arguments = "/c net stop MagicRemoteService & net start MagicRemoteService";
									pRestart.StartInfo.WindowStyle = System.Diagnostics.ProcessWindowStyle.Hidden;
									pRestart.StartInfo.CreateNoWindow = true;
									pRestart.Start();
								});
								continue;
							} else {
								byte[] buf = System.Text.Encoding.UTF8.GetBytes(GetSettingsHtml());
								ctx.Response.ContentType = "text/html; charset=utf-8";
								ctx.Response.ContentLength64 = buf.Length;
								ctx.Response.OutputStream.Write(buf, 0, buf.Length);
							}
						} catch(System.Exception ex) {
							Service.LogIfDebug("HTTP request error: " + ex.Message);
						} finally {
							ctx.Response.Close();
						}
					}
				}
			} catch(System.Exception ex) {
				Service.Warn("HTTP settings server error: " + ex.Message);
			} finally {
				hlSettings?.Stop();
				hlSettings?.Close();
			}
		}
		private static string GetSettingsHtml() {
			string strHtmlFile = System.IO.Path.Combine(System.IO.Path.GetDirectoryName(System.Reflection.Assembly.GetExecutingAssembly().Location), "settings-ui.html");
			if(System.IO.File.Exists(strHtmlFile)) {
				return System.IO.File.ReadAllText(strHtmlFile);
			}
			return @"<!DOCTYPE html><html><body><h1>MagicRemoteService</h1><p>settings-ui.html not found. Place it next to MagicRemoteService.exe.</p></body></html>";
		}
		private void ThreadSsdp() {
			try {
				using(System.Net.Sockets.UdpClient udpSsdp = new System.Net.Sockets.UdpClient()) {
					udpSsdp.ExclusiveAddressUse = false;
					udpSsdp.Client.SetSocketOption(System.Net.Sockets.SocketOptionLevel.Socket, System.Net.Sockets.SocketOptionName.ReuseAddress, true);
					udpSsdp.Client.Bind(new System.Net.IPEndPoint(System.Net.IPAddress.Any, SSDP_PORT));
					udpSsdp.JoinMulticastGroup(System.Net.IPAddress.Parse(SSDP_MULTICAST));

					while(!Service.mreStop.WaitOne(System.TimeSpan.Zero)) {
						if(udpSsdp.Available > 0) {
							System.Net.IPEndPoint epRemote = new System.Net.IPEndPoint(System.Net.IPAddress.Any, 0);
							byte[] data = udpSsdp.Receive(ref epRemote);
							string request = System.Text.Encoding.UTF8.GetString(data);
							if(request.Contains("M-SEARCH") && request.Contains(SSDP_USN)) {
								string localIp = ((System.Net.IPEndPoint)udpSsdp.Client.LocalEndPoint).Address.ToString();
								// Determine our IP that can reach the requester
								using(System.Net.Sockets.Socket tempSock = new System.Net.Sockets.Socket(System.Net.Sockets.AddressFamily.InterNetwork, System.Net.Sockets.SocketType.Dgram, System.Net.Sockets.ProtocolType.Udp)) {
									tempSock.Connect(epRemote.Address, 1);
									localIp = ((System.Net.IPEndPoint)tempSock.LocalEndPoint).Address.ToString();
								}
								string response = "HTTP/1.1 200 OK\r\n" +
									"ST: " + SSDP_USN + "\r\n" +
									"USN: " + SSDP_USN + "\r\n" +
									"LOCATION: ws://" + localIp + ":" + this.iPort + "\r\n" +
									"CACHE-CONTROL: max-age=1800\r\n" +
									"SERVER: MagicRemoteService/1.0\r\n\r\n";
								byte[] responseBytes = System.Text.Encoding.UTF8.GetBytes(response);
								udpSsdp.Send(responseBytes, responseBytes.Length, epRemote);
								Service.LogIfDebug("SSDP: Responded to M-SEARCH from " + epRemote.Address);
							}
						} else {
							System.Threading.Thread.Sleep(100);
						}
					}
				}
			} catch(System.Exception ex) {
				Service.Warn("SSDP thread error: " + ex.Message);
			}
		}
		private void ThreadServer() {

			try {
				switch(this.stType) {
					case ServiceType.Server:

						System.Net.Sockets.Socket socServer = new System.Net.Sockets.Socket(System.Net.Sockets.AddressFamily.InterNetwork, System.Net.Sockets.SocketType.Stream, System.Net.Sockets.ProtocolType.Tcp);
						socServer.Bind(new System.Net.IPEndPoint(System.Net.IPAddress.Any, this.iPort));
						socServer.Listen(10);
						System.Threading.AutoResetEvent areServerAcceptAsyncCompleted = new System.Threading.AutoResetEvent(false);
						void ServerAcceptAsyncCompleted(object o, System.Net.Sockets.SocketAsyncEventArgs e) {
							areServerAcceptAsyncCompleted.Set();
						};
						System.Net.Sockets.SocketAsyncEventArgs eaServerAcceptAsync = new System.Net.Sockets.SocketAsyncEventArgs();
						eaServerAcceptAsync.Completed += ServerAcceptAsyncCompleted;
						if(!socServer.AcceptAsync(eaServerAcceptAsync)) {
							ServerAcceptAsyncCompleted(socServer, eaServerAcceptAsync);
						}
						System.Net.Sockets.Socket socClientToSend = null;

						System.IO.Pipes.NamedPipeServerStream psServer = new System.IO.Pipes.NamedPipeServerStream("{2DCF2389-4969-483D-AA13-58FD8DDDD2D5}", System.IO.Pipes.PipeDirection.Out, 1, System.IO.Pipes.PipeTransmissionMode.Message, System.IO.Pipes.PipeOptions.Asynchronous, 4096, 4096);

						System.Diagnostics.Process pClient = null;
						System.Threading.AutoResetEvent areWaitForExitExited = new System.Threading.AutoResetEvent(false);
						void ClientWaitForExitExited(object o, System.EventArgs e) {
							areWaitForExitExited.Set();
						};

						System.Threading.WaitHandle[] tabEventServer = new System.Threading.WaitHandle[] {
							Service.mreStop,
							Service.areSessionChanged,
							Service.ewhClientConnecting,
							areServerAcceptAsyncCompleted,
							areWaitForExitExited
						};
						do {
							switch(System.Threading.WaitHandle.WaitAny(tabEventServer, -1)) {
								case 0:
									break;
								case 1:
									if(pClient != null && psServer.IsConnected && Service.ewhClientStarted.WaitOne(System.TimeSpan.Zero) && !pClient.HasExited) {
										Service.ewhSessionChanged.Set();
									} else if(socClientToSend != null) {
										if(!psServer.IsConnected || pClient == null || pClient.HasExited) {
											areWaitForExitExited.Reset();
											OpenUserInteractiveProcess(System.Reflection.Assembly.GetExecutingAssembly().Location, "-c");
										}
									}
									break;
								case 2:
									if(pClient != null) {
										pClient.EnableRaisingEvents = false;
										pClient.Exited -= ClientWaitForExitExited;
										pClient.Close();
										pClient.Dispose();
									}
									if(psServer.IsConnected) {
										psServer.Disconnect();
									}
									psServer.WaitForConnection();
									pClient = psServer.GetClientProcess();
									pClient.Exited += ClientWaitForExitExited;
									pClient.EnableRaisingEvents = true;
									if(socClientToSend != null) {
										Service.SerializeSocketInfo(psServer, socClientToSend.DuplicateAndClose(pClient.Id));
										Service.ewhServerMessage.Set();
										socClientToSend.Dispose();
										socClientToSend = null;
									}
									break;
								case 3:
									if(socClientToSend != null) {
										socClientToSend.Close();
										socClientToSend.Dispose();
										socClientToSend = null;
									}
									if(pClient != null && psServer.IsConnected && Service.ewhClientStarted.WaitOne(System.TimeSpan.Zero) && !pClient.HasExited) {
										Service.SerializeSocketInfo(psServer, eaServerAcceptAsync.AcceptSocket.DuplicateAndClose(pClient.Id));
										Service.ewhServerMessage.Set();
										eaServerAcceptAsync.AcceptSocket.Dispose();
									} else {
										socClientToSend = eaServerAcceptAsync.AcceptSocket;
										if(!psServer.IsConnected || pClient == null || pClient.HasExited) {
											areWaitForExitExited.Reset();
											OpenUserInteractiveProcess(System.Reflection.Assembly.GetExecutingAssembly().Location, "-c");
										}
									}
									eaServerAcceptAsync.AcceptSocket = null;
									if(!socServer.AcceptAsync(eaServerAcceptAsync)) {
										ServerAcceptAsyncCompleted(socServer, eaServerAcceptAsync);
									}
									break;
								case 4:
									if(socClientToSend != null) {
										OpenUserInteractiveProcess(System.Reflection.Assembly.GetExecutingAssembly().Location, "-c");
									}
									break;
								default:
									throw new System.Exception("Unmanaged handle error");
							}
						} while(!Service.mreStop.WaitOne(System.TimeSpan.Zero));

						Service.ewhServerDisconnecting.Set();

						if(pClient != null) {
							pClient.EnableRaisingEvents = false;
							pClient.Exited -= ClientWaitForExitExited;
							pClient.Close();
							pClient.Dispose();
						}
						areWaitForExitExited.Close();
						areWaitForExitExited.Dispose();

						if(psServer.IsConnected) {
							psServer.Disconnect();
						}
						psServer.Close();
						psServer.Dispose();

						eaServerAcceptAsync.Completed -= ServerAcceptAsyncCompleted;
						eaServerAcceptAsync.Dispose();
						areServerAcceptAsyncCompleted.Close();
						areServerAcceptAsyncCompleted.Dispose();
						socServer.Close();
						socServer.Dispose();

						break;
					case ServiceType.Both:

						System.Net.Sockets.Socket socBoth = new System.Net.Sockets.Socket(System.Net.Sockets.AddressFamily.InterNetwork, System.Net.Sockets.SocketType.Stream, System.Net.Sockets.ProtocolType.Tcp);
						socBoth.Bind(new System.Net.IPEndPoint(System.Net.IPAddress.Any, this.iPort));
						socBoth.Listen(10);
						System.Threading.AutoResetEvent areBothAcceptAsyncCompleted = new System.Threading.AutoResetEvent(false);
						void BothAcceptAsyncCompleted(object o, System.Net.Sockets.SocketAsyncEventArgs e) {
							areBothAcceptAsyncCompleted.Set();
						};
						System.Net.Sockets.SocketAsyncEventArgs eaBothAcceptAsync = new System.Net.Sockets.SocketAsyncEventArgs();
						eaBothAcceptAsync.Completed += BothAcceptAsyncCompleted;
						if(!socBoth.AcceptAsync(eaBothAcceptAsync)) {
							BothAcceptAsyncCompleted(socBoth, eaBothAcceptAsync);
						}

						System.Collections.Generic.List<System.Threading.Thread> liClientBoth = new System.Collections.Generic.List<System.Threading.Thread>();

						System.Threading.WaitHandle[] tabEventBoth = new System.Threading.WaitHandle[] {
							Service.mreStop,
							Service.ewhServerStarted,
							areBothAcceptAsyncCompleted
						};
						do {
							switch(System.Threading.WaitHandle.WaitAny(tabEventBoth, -1)) {
								case 0:
									break;
								case 1:
									Service.mreStop.Set();
									System.Threading.Tasks.Task.Run(delegate () {
										this.ServiceStop();
										this.ServiceStart();
									});
									break;
								case 2:
									System.Net.Sockets.Socket socClient = eaBothAcceptAsync.AcceptSocket;

									System.Threading.Thread thrClient = new System.Threading.Thread(delegate () {
										this.ThreadClient(socClient);
									});
									thrClient.Start();
									liClientBoth.Add(thrClient);

									eaBothAcceptAsync.AcceptSocket = null;
									if(!socBoth.AcceptAsync(eaBothAcceptAsync)) {
										BothAcceptAsyncCompleted(socBoth, eaBothAcceptAsync);
									}
									break;
								default:
									throw new System.Exception("Unmanaged handle error");
							}
						} while(!Service.mreStop.WaitOne(System.TimeSpan.Zero));

						liClientBoth.RemoveAll(delegate (System.Threading.Thread thr) {
							thr.Join(System.TimeSpan.FromSeconds(10));
							return true;
						});

						eaBothAcceptAsync.Completed -= BothAcceptAsyncCompleted;
						eaBothAcceptAsync.Dispose();
						areBothAcceptAsyncCompleted.Close();
						areBothAcceptAsyncCompleted.Dispose();
						socBoth.Close();
						socBoth.Dispose();
						break;
					case ServiceType.Client:

						System.IO.Pipes.NamedPipeClientStream psClient = new System.IO.Pipes.NamedPipeClientStream(".", "{2DCF2389-4969-483D-AA13-58FD8DDDD2D5}", System.IO.Pipes.PipeDirection.In, System.IO.Pipes.PipeOptions.Asynchronous);
						Service.ewhClientConnecting.Set();
						psClient.Connect();
						System.Diagnostics.Process pServer = psClient.GetServerProcess();
						System.Collections.Generic.List<System.Threading.Thread> liClient = new System.Collections.Generic.List<System.Threading.Thread>();

						System.Threading.WaitHandle[] tabEventClient = new System.Threading.WaitHandle[] {
							Service.mreStop,
							Service.ewhSessionChanged,
							Service.ewhServerDisconnecting,
							Service.ewhServerMessage
						};
						do {
							switch(System.Threading.WaitHandle.WaitAny(tabEventClient, -1)) {
								case 0:
									break;
								case 1:
									Service.mreStop.Set();
									System.Threading.Tasks.Task.Run(delegate () {
										this.ServiceStop();
										System.Windows.Forms.Application.Exit();
									});
									break;
								case 2:
									Service.mreStop.Set();
									if(!(System.Array.IndexOf<string>(System.Environment.GetCommandLineArgs(), "-c") < 0) && System.Windows.Forms.Application.OpenForms.Count == 0) {
										System.Threading.Tasks.Task.Run(delegate () {
											this.ServiceStop();
											System.Windows.Forms.Application.Exit();
										});
									} else {
										System.Threading.Tasks.Task.Run(delegate () {
											this.ServiceStop();
											this.ServiceStart();
										});
									}
									break;
								case 3:
									System.Net.Sockets.SocketInformation si = Service.DeserializeSocketInfo(psClient);
									System.Net.Sockets.Socket socClient = new System.Net.Sockets.Socket(si);

									System.Threading.Thread thrClient = new System.Threading.Thread(delegate () {
										this.ThreadClient(socClient);
									});
									thrClient.Start();
									liClient.Add(thrClient);
									break;
								default:
									throw new System.Exception("Unmanaged handle error");
							}
						} while(!Service.mreStop.WaitOne(System.TimeSpan.Zero));

						psClient.Close();
						psClient.Dispose();

						liClient.RemoveAll(delegate (System.Threading.Thread thr) {
							thr.Join(System.TimeSpan.FromSeconds(10));
							return true;
						});
						break;
				}
			} catch(System.IO.IOException eException) {
				Service.Error("IO error in server thread: " + eException.ToString());
				System.Threading.Tasks.Task.Run(delegate () {
					this.ServiceStop();
					this.ServiceStart();
				});
			} catch(System.Net.Sockets.SocketException eException) {
				Service.Error("Socket error in server thread: " + eException.ToString());
				System.Threading.Tasks.Task.Run(delegate () {
					this.ServiceStop();
					this.ServiceStart();
				});
			} catch(System.ObjectDisposedException eException) {
				Service.Error("Object disposed in server thread: " + eException.ToString());
				System.Threading.Tasks.Task.Run(delegate () {
					this.ServiceStop();
					this.ServiceStart();
				});
			} catch(System.InvalidOperationException eException) {
				Service.Error("Invalid operation in server thread: " + eException.ToString());
				System.Threading.Tasks.Task.Run(delegate () {
					this.ServiceStop();
					this.ServiceStart();
				});
			}
		}
		private void ThreadClient(System.Net.Sockets.Socket socClient) {
			try {
				Service.Log("Socket accepted [" + socClient.GetHashCode() + "]");
				byte[] tabData = new byte[4096];
				System.Threading.AutoResetEvent areClientReceiveAsyncCompleted = new System.Threading.AutoResetEvent(false);
				void ClientReceiveAsyncCompleted(object o, System.Net.Sockets.SocketAsyncEventArgs e) {
					areClientReceiveAsyncCompleted.Set();
				};
				System.Net.Sockets.SocketAsyncEventArgs eaClientReceiveAsync = new System.Net.Sockets.SocketAsyncEventArgs();
				eaClientReceiveAsync.SetBuffer(tabData, 0, tabData.Length);
				eaClientReceiveAsync.Completed += ClientReceiveAsyncCompleted;
				if(!socClient.ReceiveAsync(eaClientReceiveAsync)) {
					ClientReceiveAsyncCompleted(socClient, eaClientReceiveAsync);
				}

				System.Threading.ManualResetEvent mreClientStop = new System.Threading.ManualResetEvent(false);

				System.Timers.Timer tUserInput = new System.Timers.Timer {
					Interval = 500,
					AutoReset = true
				};
				tUserInput.Elapsed += delegate (object oSource, System.Timers.ElapsedEventArgs eElapsed) {
					WinApi.LastInputInfo lii = new WinApi.LastInputInfo();
					lii.cbSize = (uint)System.Runtime.InteropServices.Marshal.SizeOf(lii);
					if(!WinApi.User32.GetLastInputInfo(ref lii)) {
					} else if((unchecked((uint)System.Environment.TickCount - lii.dwTime)) < 500) {
						using(System.Diagnostics.Process pProcessAbort = new System.Diagnostics.Process()) {
							pProcessAbort.StartInfo.FileName = "shutdown";
							pProcessAbort.StartInfo.Arguments = "/a";
							pProcessAbort.StartInfo.UseShellExecute = false;
							pProcessAbort.StartInfo.WindowStyle = System.Diagnostics.ProcessWindowStyle.Hidden;
							pProcessAbort.Start();
						}
						tUserInput.Stop();
						Service.Log("Client user input activity on socket [" + socClient.GetHashCode() + "]");
					}
				};
				System.Timers.Timer tPongUserInput = new System.Timers.Timer {
					Interval = 5000,
					AutoReset = false
				};
				tPongUserInput.Elapsed += delegate (object oSource, System.Timers.ElapsedEventArgs eElapsed) {
					socClient.Send(Service.tabClose);
					mreClientStop.Set();
					Service.Warn("Client timeout pong inactivity on socket [" + socClient.GetHashCode() + "]");
				};
				System.Timers.Timer tInactivity = new System.Timers.Timer {
					Interval = System.Math.Max(1000, this.iTimeoutInactivity - 300000),
					AutoReset = false
				};
				tInactivity.Elapsed += delegate (object oSource, System.Timers.ElapsedEventArgs eElapsed) {
					socClient.Send(Service.tabPingUserInput);
					tPongUserInput.Start();
					Service.Log("Client timeout inactivity on socket [" + socClient.GetHashCode() + "]");
				};

				System.Timers.Timer tVideoInput = new System.Timers.Timer {
					Interval = System.Math.Max(1000, this.iTimeoutVideoInput - 300000),
					AutoReset = false
				};
				tVideoInput.Elapsed += delegate (object oSource, System.Timers.ElapsedEventArgs eElapsed) {
					socClient.Send(Service.tabPingUserInput);
					tPongUserInput.Start();
					Service.Log("Client timeout video input on socket [" + socClient.GetHashCode() + "]");
				};

				System.Timers.Timer tPong = new System.Timers.Timer {
					Interval = 5000,
					AutoReset = false
				};
				tPong.Elapsed += delegate (object oSource, System.Timers.ElapsedEventArgs eElapsed) {
					socClient.Send(Service.tabClose);
					mreClientStop.Set();
					Service.Warn("Client timeout pong on socket [" + socClient.GetHashCode() + "]");
				};
				System.Timers.Timer tPing = new System.Timers.Timer {
					Interval = 30000,
					AutoReset = true
				};
				tPing.Elapsed += delegate (object oSource, System.Timers.ElapsedEventArgs eElapsed) {
					socClient.Send(Service.tabPing);
					tPong.Start();
				};

				void PowerSettingNotificationArrived(WinApi.PowerBroadcastSetting pbs) {
					if(pbs.PowerSetting == WinApi.User32.GUID_MONITOR_POWER_ON) {
						switch(pbs.Data) {
							case 0:
								if(this.bVideoInput) {
									tVideoInput.Start();
								}
								break;
							case 1:
								if(this.bVideoInput) {
									tVideoInput.Stop();
								}
								break;
						}
					}
				};
				MagicRemoteService.Application.naehPowerSettingNotificationArrived += PowerSettingNotificationArrived;

				System.Collections.Generic.Dictionary<ushort, WinApi.Input[]> dBindDown = new System.Collections.Generic.Dictionary<ushort, WinApi.Input[]>();
				System.Collections.Generic.Dictionary<ushort, WinApi.Input[]> dBindUp = new System.Collections.Generic.Dictionary<ushort, WinApi.Input[]>();
				System.Collections.Generic.Dictionary<ushort, byte[][]> dBindActionDown = new System.Collections.Generic.Dictionary<ushort, byte[][]>();
				System.Collections.Generic.Dictionary<ushort, string[]> dBindCommandDown = new System.Collections.Generic.Dictionary<ushort, string[]>();
				foreach(System.Collections.Generic.KeyValuePair<ushort, Bind[]> kvp in this.dBind) {
					if(kvp.Value != null) {
						System.Collections.Generic.List<WinApi.Input> liBindDown = new System.Collections.Generic.List<WinApi.Input>();
						System.Collections.Generic.List<WinApi.Input> liBindUp = new System.Collections.Generic.List<WinApi.Input>();
						System.Collections.Generic.List<byte[]> liBindActionDown = new System.Collections.Generic.List<byte[]>();
						System.Collections.Generic.List<string> liBindCommandDown = new System.Collections.Generic.List<string>();
						foreach(Bind b in kvp.Value) {
							switch(b) {
								case MagicRemoteService.BindMouse bm:
									switch(bm.bmvValue) {
										case BindMouseValue.Left:
											liBindDown.Add(new WinApi.Input {
												type = WinApi.InputType.INPUT_MOUSE,
												u = new WinApi.InputDummyUnionName {
													mi = new WinApi.MouseInput {
														dwFlags = WinApi.MouseInputFlags.MOUSEEVENTF_LEFTDOWN,
														dwExtraInfo = System.IntPtr.Zero
													}
												}
											});
											liBindUp.Add(new WinApi.Input {
												type = WinApi.InputType.INPUT_MOUSE,
												u = new WinApi.InputDummyUnionName {
													mi = new WinApi.MouseInput {
														dwFlags = WinApi.MouseInputFlags.MOUSEEVENTF_LEFTUP,
														dwExtraInfo = System.IntPtr.Zero
													}
												}
											});
											break;
										case BindMouseValue.Right:
											liBindDown.Add(new WinApi.Input {
												type = WinApi.InputType.INPUT_MOUSE,
												u = new WinApi.InputDummyUnionName {
													mi = new WinApi.MouseInput {
														dwFlags = WinApi.MouseInputFlags.MOUSEEVENTF_RIGHTDOWN,
														dwExtraInfo = System.IntPtr.Zero
													}
												}
											});
											liBindUp.Add(new WinApi.Input {
												type = WinApi.InputType.INPUT_MOUSE,
												u = new WinApi.InputDummyUnionName {
													mi = new WinApi.MouseInput {
														dwFlags = WinApi.MouseInputFlags.MOUSEEVENTF_RIGHTUP,
														dwExtraInfo = System.IntPtr.Zero
													}
												}
											});
											break;
										case BindMouseValue.Middle:
											liBindDown.Add(new WinApi.Input {
												type = WinApi.InputType.INPUT_MOUSE,
												u = new WinApi.InputDummyUnionName {
													mi = new WinApi.MouseInput {
														dwFlags = WinApi.MouseInputFlags.MOUSEEVENTF_MIDDLEDOWN,
														dwExtraInfo = System.IntPtr.Zero
													}
												}
											});
											liBindUp.Add(new WinApi.Input {
												type = WinApi.InputType.INPUT_MOUSE,
												u = new WinApi.InputDummyUnionName {
													mi = new WinApi.MouseInput {
														dwFlags = WinApi.MouseInputFlags.MOUSEEVENTF_MIDDLEUP,
														dwExtraInfo = System.IntPtr.Zero
													}
												}
											});
											break;
									}
									break;
								case MagicRemoteService.BindKeyboard bk:
									liBindDown.Add(new WinApi.Input {
										type = WinApi.InputType.INPUT_KEYBOARD,
										u = new WinApi.InputDummyUnionName {
											ki = new WinApi.KeybdInput {
												wVk = bk.ucVirtualKey,
												wScan = bk.ucScanCode,
												dwFlags = bk.bExtended ? (WinApi.KeybdInputFlags.KEYEVENTF_EXTENDEDKEY | WinApi.KeybdInputFlags.KEYEVENTF_KEYDOWN) : WinApi.KeybdInputFlags.KEYEVENTF_KEYDOWN,
												dwExtraInfo = System.IntPtr.Zero
											}
										}
									});
									liBindUp.Add(new WinApi.Input {
										type = WinApi.InputType.INPUT_KEYBOARD,
										u = new WinApi.InputDummyUnionName {
											ki = new WinApi.KeybdInput {
												wVk = bk.ucVirtualKey,
												wScan = bk.ucScanCode,
												dwFlags = bk.bExtended ? (WinApi.KeybdInputFlags.KEYEVENTF_EXTENDEDKEY | WinApi.KeybdInputFlags.KEYEVENTF_KEYUP) : WinApi.KeybdInputFlags.KEYEVENTF_KEYUP,
												dwExtraInfo = System.IntPtr.Zero
											}
										}
									});
									break;
								case MagicRemoteService.BindAction ba:
									liBindActionDown.Add(new byte[] {
										(0b1000 << 4) | (0x2 << 0),
										0x01,
										(byte)ba.bavValue
									});
									break;
								case MagicRemoteService.BindCommand bc:
									liBindCommandDown.Add(bc.strCommand);
									break;
							}
						}
						if(liBindDown.Count > 0) {
							dBindDown.Add(kvp.Key, liBindDown.ToArray());
						}
						if(liBindUp.Count > 0) {
							dBindUp.Add(kvp.Key, liBindUp.ToArray());
						}
						if(liBindActionDown.Count > 0) {
							dBindActionDown.Add(kvp.Key, liBindActionDown.ToArray());
						}
						if(liBindCommandDown.Count > 0) {
							dBindCommandDown.Add(kvp.Key, liBindCommandDown.ToArray());
						}
					}
				};

				WinApi.Input[] piPositionRelative = new WinApi.Input[] {
					new WinApi.Input {
						type = WinApi.InputType.INPUT_MOUSE,
						u = new WinApi.InputDummyUnionName {
							mi = new WinApi.MouseInput {
								dwFlags = WinApi.MouseInputFlags.MOUSEEVENTF_MOVE,
								dwExtraInfo = System.IntPtr.Zero
							}
						}
					}
				};
				WinApi.Input[] piPositionAbsolute = new WinApi.Input[] {
					new WinApi.Input {
						type = WinApi.InputType.INPUT_MOUSE,
						u = new WinApi.InputDummyUnionName {
							mi = new WinApi.MouseInput {
								dwFlags = WinApi.MouseInputFlags.MOUSEEVENTF_ABSOLUTE | WinApi.MouseInputFlags.MOUSEEVENTF_VIRTUALDESK | WinApi.MouseInputFlags.MOUSEEVENTF_MOVE,
								dwExtraInfo = System.IntPtr.Zero
							}
						}
					}
				};
				WinApi.Input[] piWheel = new WinApi.Input[] {
					new WinApi.Input {
						type = WinApi.InputType.INPUT_MOUSE,
						u = new WinApi.InputDummyUnionName {
							mi = new WinApi.MouseInput {
								dwFlags = WinApi.MouseInputFlags.MOUSEEVENTF_WHEEL,
								dwExtraInfo = System.IntPtr.Zero
							}
						}
					}
				};
				WinApi.Input[] piUnicode = new WinApi.Input[] {
					new WinApi.Input {
						type = WinApi.InputType.INPUT_KEYBOARD,
						u = new WinApi.InputDummyUnionName {
							ki = new WinApi.KeybdInput {
								dwFlags = WinApi.KeybdInputFlags.KEYEVENTF_UNICODE | WinApi.KeybdInputFlags.KEYEVENTF_KEYDOWN,
								dwExtraInfo = System.IntPtr.Zero
							}
						}
					}, new WinApi.Input {
						type = WinApi.InputType.INPUT_KEYBOARD,
						u = new WinApi.InputDummyUnionName {
							ki = new WinApi.KeybdInput {
								dwFlags = WinApi.KeybdInputFlags.KEYEVENTF_UNICODE | WinApi.KeybdInputFlags.KEYEVENTF_KEYUP,
								dwExtraInfo = System.IntPtr.Zero
							}
						}
					}
				};

				MagicRemoteService.Screen scrDisplay = MagicRemoteService.Screen.PrimaryScreen;
				System.Threading.Tasks.Task.Run(delegate () {
					System.Net.IPAddress iaClient = ((System.Net.IPEndPoint)socClient.RemoteEndPoint).Address;
					MagicRemoteService.WebOSCLIDevice wocdClient = System.Array.Find<MagicRemoteService.WebOSCLIDevice>(MagicRemoteService.WebOSCLI.SetupDeviceList(), delegate (MagicRemoteService.WebOSCLIDevice wocd) {
						return wocd.DeviceInfo.IP.Equals(iaClient);
					});
					if(wocdClient != null) {
						Microsoft.Win32.RegistryKey rkMagicRemoteServiceDevice = (MagicRemoteService.Program.bElevated ? Microsoft.Win32.Registry.LocalMachine : Microsoft.Win32.Registry.CurrentUser).OpenSubKey(@"Software\MagicRemoteService\Device\" + wocdClient.Name);
						if(rkMagicRemoteServiceDevice != null && MagicRemoteService.Screen.AllScreen.TryGetValue((uint)(int)rkMagicRemoteServiceDevice.GetValue("Display", 0), out MagicRemoteService.Screen scr) && scr.Active) {
							scrDisplay = scr;
						}
					}
				});

				System.Threading.WaitHandle[] tabEvent = new System.Threading.WaitHandle[] {
					Service.mreStop,
					mreClientStop,
					areClientReceiveAsyncCompleted
				};
				switch(System.Threading.WaitHandle.WaitAny(tabEvent, -1, true)) {
					case 0:
						break;
					case 1:
						break;
					case 2:
						ulong ulLenMessage = (ulong)eaClientReceiveAsync.BytesTransferred;
						if(tabData[0] == 'G' && tabData[1] == 'E' && tabData[2] == 'T') {
							string strHandshake = System.Text.Encoding.UTF8.GetString(tabData, 0, (int)ulLenMessage);
							System.Text.RegularExpressions.Match mWebSocketKey = System.Text.RegularExpressions.Regex.Match(strHandshake, "Sec-WebSocket-Key: (.*)\r\n");
							if(!mWebSocketKey.Success || string.IsNullOrEmpty(mWebSocketKey.Groups[1].Value)) {
								mreClientStop.Set();
								Service.Warn("Invalid WebSocket handshake: missing Sec-WebSocket-Key on socket [" + socClient.GetHashCode() + "]");
							} else {
								using(System.Security.Cryptography.SHA1 sha1 = System.Security.Cryptography.SHA1.Create()) {
									socClient.Send(System.Text.Encoding.UTF8.GetBytes(
										"HTTP/1.1 101 Switching Protocols\r\n" +
										"Connection: Upgrade\r\n" +
										"Upgrade: websocket\r\n" +
										"Sec-WebSocket-Accept: " + System.Convert.ToBase64String(sha1.ComputeHash(System.Text.Encoding.UTF8.GetBytes(mWebSocketKey.Groups[1].Value.Trim() + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"))) + "\r\n\r\n"));
								}

								//TODO Something to ask TV if cursor visible
								Service.Log("Client connected on socket [" + socClient.GetHashCode() + "]");
							System.Threading.Interlocked.Increment(ref Service.iConnectedClients);
							try { Service.strConnectedClientIp = ((System.Net.IPEndPoint)socClient.RemoteEndPoint).Address.ToString(); } catch {}
								tPing.Start();
								if(this.bInactivity) {
									tInactivity.Start();
								}
							}
						} else {
							mreClientStop.Set();
							Service.Warn("Connexion refused on socket [" + socClient.GetHashCode() + "]");
						}

						if(!socClient.ReceiveAsync(eaClientReceiveAsync)) {
							ClientReceiveAsyncCompleted(socClient, eaClientReceiveAsync);
						}
						System.Threading.Thread.Sleep(1);
						break;
					default:
						throw new System.Exception("Unmanaged handle error");
				}
				while(!mreClientStop.WaitOne(System.TimeSpan.Zero)) {
					switch(System.Threading.WaitHandle.WaitAny(tabEvent, -1, true)) {
						case 0:
							socClient.Send(Service.tabClose);
							mreClientStop.Set();
							break;
						case 1:
							break;
						case 2:
							ulong ulLenMessage = (ulong)eaClientReceiveAsync.BytesTransferred;
							ulong ulOffsetFrame = 0;
							while(!(ulOffsetFrame == ulLenMessage)) {
								// Bounds check: need at least 2 bytes for frame header
								if(ulOffsetFrame + 1 >= ulLenMessage) {
									Service.Warn("Incomplete WebSocket frame header on socket [" + socClient.GetHashCode() + "]");
									break;
								}
								bool bFin = (tabData[ulOffsetFrame] & 0b10000000) == 0b10000000;
								bool bRsv1 = (tabData[ulOffsetFrame] & 0b01000000) == 0b01000000;
								bool bRsv2 = (tabData[ulOffsetFrame] & 0b00100000) == 0b00100000;
								bool bRsv3 = (tabData[ulOffsetFrame] & 0b00010000) == 0b00010000;
								byte ucOpcode = (byte)(tabData[ulOffsetFrame] & 0b00001111);

								bool bMask = (tabData[ulOffsetFrame + 1] & 0b10000000) == 0b10000000;
								ulong ulLenData;
								ulong ulOffsetMask;
								if((tabData[ulOffsetFrame + 1] & 0b01111111) == 0b01111111) {
									if(ulOffsetFrame + 9 >= ulLenMessage) {
										Service.Warn("Incomplete 64-bit length in WebSocket frame on socket [" + socClient.GetHashCode() + "]");
										break;
									}
									ulLenData = System.BitConverter.ToUInt64(new byte[] { tabData[ulOffsetFrame + 9], tabData[ulOffsetFrame + 8], tabData[ulOffsetFrame + 7], tabData[ulOffsetFrame + 6], tabData[ulOffsetFrame + 5], tabData[ulOffsetFrame + 4], tabData[ulOffsetFrame + 3], tabData[ulOffsetFrame + 2] }, 0);
									ulOffsetMask = ulOffsetFrame + 10;
								} else if((tabData[ulOffsetFrame + 1] & 0b01111111) == 0b01111110) {
									if(ulOffsetFrame + 3 >= ulLenMessage) {
										Service.Warn("Incomplete 16-bit length in WebSocket frame on socket [" + socClient.GetHashCode() + "]");
										break;
									}
									ulLenData = System.BitConverter.ToUInt16(new byte[] { tabData[ulOffsetFrame + 3], tabData[ulOffsetFrame + 2] }, 0);
									ulOffsetMask = ulOffsetFrame + 4;
								} else {
									ulLenData = (byte)(tabData[ulOffsetFrame + 1] & 0b01111111);
									ulOffsetMask = ulOffsetFrame + 2;
								}

								ulong ulOffsetData;
								if(bMask) {
									ulOffsetData = ulOffsetMask + 4;
								} else {
									ulOffsetData = ulOffsetMask;
								}

								// Validate frame data fits within received message and buffer
								if(ulOffsetData + ulLenData > (ulong)tabData.Length || ulOffsetData + ulLenData > ulLenMessage) {
									Service.Warn("WebSocket frame data exceeds buffer bounds on socket [" + socClient.GetHashCode() + "]");
									break;
								}
								if(bMask) {
									if(ulOffsetMask + 3 >= (ulong)tabData.Length) {
										Service.Warn("WebSocket mask exceeds buffer on socket [" + socClient.GetHashCode() + "]");
										break;
									}
									for(ulong ul = 0; ul < ulLenData; ul++) {
										tabData[ulOffsetData + ul] ^= tabData[ulOffsetMask + (ul % 4)];
									}
								}
								if(!bFin) {
									Service.Warn("Unable to process split frame on socket [" + socClient.GetHashCode() + "]");
								} else {
									switch(ucOpcode) {
										case (byte)MagicRemoteService.WebSocketOpCode.Continuation:
											Service.Warn("Unable to process split frame on socket [" + socClient.GetHashCode() + "]");
											break;
										case (byte)MagicRemoteService.WebSocketOpCode.Text:
											if(ulLenData != 0) {
												Service.Warn("Unprocessed text message [" + System.Text.Encoding.UTF8.GetString(tabData, (int)ulOffsetData, (int)ulLenData) + "]");
											}
											break;
										case (byte)MagicRemoteService.WebSocketOpCode.Binary:
											tPing.Stop();
											tPing.Start();
											if(this.bInactivity) {
												tInactivity.Stop();
												tInactivity.Start();
											}
											if(ulLenData != 0) {
												switch(tabData[ulOffsetData + 0]) {
													case (byte)MagicRemoteService.MessageType.PositionRelative:
											if(ulLenData < 5) break;
														piPositionRelative[0].u.mi.dx = System.BitConverter.ToInt16(tabData, (int)ulOffsetData + 1);
														piPositionRelative[0].u.mi.dy = System.BitConverter.ToInt16(tabData, (int)ulOffsetData + 3);
														Service.SendInputAdmin(piPositionRelative);
														break;
													case (byte)MagicRemoteService.MessageType.PositionAbsolute:
											if(ulLenData < 5) break;
														piPositionAbsolute[0].u.mi.dx = ((scrDisplay.Bounds.X + ((scrDisplay.Bounds.Width * System.BitConverter.ToUInt16(tabData, (int)ulOffsetData + 1)) / 1920)) * 65535) / MagicRemoteService.Screen.DesktopBounds.Width;
														piPositionAbsolute[0].u.mi.dy = ((scrDisplay.Bounds.Y + ((scrDisplay.Bounds.Height * System.BitConverter.ToUInt16(tabData, (int)ulOffsetData + 3)) / 1080)) * 65535) / MagicRemoteService.Screen.DesktopBounds.Height;
														Service.SendInputAdmin(piPositionAbsolute);
														break;
													case (byte)MagicRemoteService.MessageType.Wheel:
											if(ulLenData < 3) break;
														piWheel[0].u.mi.mouseData = (uint)(-System.BitConverter.ToInt16(tabData, (int)ulOffsetData + 1) * 3);
														Service.SendInputAdmin(piWheel);
														Service.LogIfDebug("Processed binary message send/wheel [0x" + System.BitConverter.ToString(tabData, (int)ulOffsetData, (int)ulLenData).Replace("-", string.Empty) + "], sY: " + (-System.BitConverter.ToInt16(tabData, (int)ulOffsetData + 1)).ToString());
														break;

													case (byte)MagicRemoteService.MessageType.Visible:
											if(ulLenData < 2) break;
														if(System.BitConverter.ToBoolean(tabData, (int)ulOffsetData + 1)) {
															MagicRemoteService.SystemCursor.SetMagicRemoteServiceSystemCursor();
															MagicRemoteService.SystemCursor.SetMagicRemoteServiceMouseSpeedAccel();
														} else {
															MagicRemoteService.SystemCursor.SetDefaultSystemCursor();
															MagicRemoteService.SystemCursor.SetDefaultMouseSpeedAccel();
														}
														Service.LogIfDebug("Processed binary message send/visible [0x" + System.BitConverter.ToString(tabData, (int)ulOffsetData, (int)ulLenData).Replace("-", string.Empty) + "], bV: " + System.BitConverter.ToBoolean(tabData, (int)ulOffsetData + 1).ToString());
														break;
													case (byte)MagicRemoteService.MessageType.Key:
											if(ulLenData < 4) break;
														ushort usCode = System.BitConverter.ToUInt16(tabData, (int)ulOffsetData + 1);
														if((tabData[ulOffsetData + 3] & 0x01) == 0x01) {
															if(dBindDown.TryGetValue(usCode, out WinApi.Input[] arrInput)) {
																Service.SendInputAdmin(arrInput);
																Service.LogIfDebug("Processed binary message send/key [0x" + System.BitConverter.ToString(tabData, (int)ulOffsetData, (int)ulLenData).Replace("-", string.Empty) + "], usC: " + System.BitConverter.ToUInt16(tabData, (int)ulOffsetData + 1).ToString() + ", bS: " + System.BitConverter.ToBoolean(tabData, (int)ulOffsetData + 3).ToString());
															} else if(dBindActionDown.TryGetValue(usCode, out byte[][] arr2Byte)) {
																foreach(byte[] arrByte in arr2Byte) {
																	socClient.Send(arrByte);
																}
																Service.LogIfDebug("Processed binary message send/key action [0x" + System.BitConverter.ToString(tabData, (int)ulOffsetData, (int)ulLenData).Replace("-", string.Empty) + "], usC: " + System.BitConverter.ToUInt16(tabData, (int)ulOffsetData + 1).ToString() + ", bS: " + System.BitConverter.ToBoolean(tabData, (int)ulOffsetData + 3).ToString());
															} else if(dBindCommandDown.TryGetValue(usCode, out string[] arrString)) {
																foreach(string strCommand in arrString) {
																	using(System.Diagnostics.Process pCommand = new System.Diagnostics.Process()) {
																		pCommand.StartInfo.FileName = "cmd";
																		pCommand.StartInfo.Arguments = "/c " + strCommand;
																		pCommand.StartInfo.UseShellExecute = false;
																		pCommand.StartInfo.CreateNoWindow = true;
																		pCommand.StartInfo.WindowStyle = System.Diagnostics.ProcessWindowStyle.Hidden;
																		pCommand.Start();
																	}
																}
																Service.LogIfDebug("Processed binary message send/key command [0x" + System.BitConverter.ToString(tabData, (int)ulOffsetData, (int)ulLenData).Replace("-", string.Empty) + "], usC: " + System.BitConverter.ToUInt16(tabData, (int)ulOffsetData + 1).ToString() + ", bS: " + System.BitConverter.ToBoolean(tabData, (int)ulOffsetData + 3).ToString());
															} else {
																Service.LogIfDebug("Unprocessed binary message send/key [0x" + System.BitConverter.ToString(tabData, (int)ulOffsetData, (int)ulLenData).Replace("-", string.Empty) + "], usC: " + System.BitConverter.ToUInt16(tabData, (int)ulOffsetData + 1).ToString() + ", bS: " + System.BitConverter.ToBoolean(tabData, (int)ulOffsetData + 3).ToString());
															}
														} else {
															if(dBindUp.TryGetValue(usCode, out WinApi.Input[] arrInput)) {
																Service.SendInputAdmin(arrInput);
																Service.LogIfDebug("Processed binary message send/key [0x" + System.BitConverter.ToString(tabData, (int)ulOffsetData, (int)ulLenData).Replace("-", string.Empty) + "], usC: " + System.BitConverter.ToUInt16(tabData, (int)ulOffsetData + 1).ToString() + ", bS: " + System.BitConverter.ToBoolean(tabData, (int)ulOffsetData + 3).ToString());
															} else {
																Service.LogIfDebug("Unprocessed binary message send/key [0x" + System.BitConverter.ToString(tabData, (int)ulOffsetData, (int)ulLenData).Replace("-", string.Empty) + "], usC: " + System.BitConverter.ToUInt16(tabData, (int)ulOffsetData + 1).ToString() + ", bS: " + System.BitConverter.ToBoolean(tabData, (int)ulOffsetData + 3).ToString());
															}
														}
														break;
													case (byte)MagicRemoteService.MessageType.Unicode:
											if(ulLenData < 3) break;
														ushort usScan = System.BitConverter.ToUInt16(tabData, (int)ulOffsetData + 1);
														piUnicode[0].u.ki.wScan = usScan;
														piUnicode[1].u.ki.wScan = usScan;
														Service.SendInputAdmin(piUnicode);
														Service.LogIfDebug("Processed binary message send/unicode [0x" + System.BitConverter.ToString(tabData, (int)ulOffsetData, (int)ulLenData).Replace("-", string.Empty) + "], usC: " + System.Text.Encoding.UTF8.GetString(tabData, (int)ulOffsetData + 1, 2));
														break;
													case (byte)MagicRemoteService.MessageType.Shutdown:
														using(System.Diagnostics.Process pProcessShutdown = new System.Diagnostics.Process()) {
															pProcessShutdown.StartInfo.FileName = "shutdown";
															pProcessShutdown.StartInfo.Arguments = "/s /t 0";
															pProcessShutdown.StartInfo.UseShellExecute = false;
															pProcessShutdown.StartInfo.WindowStyle = System.Diagnostics.ProcessWindowStyle.Hidden;
															pProcessShutdown.Start();
														}
														Service.LogIfDebug("Processed binary message send/shutdown [0x" + System.BitConverter.ToString(tabData, (int)ulOffsetData, (int)ulLenData).Replace("-", string.Empty) + "]");
														break;
													default:
														Service.Warn("Unprocessed binary message [0x" + System.BitConverter.ToString(tabData, (int)ulOffsetData, (int)ulLenData).Replace("-", string.Empty) + "]");
														break;
												}
											}
											break;
										case (byte)MagicRemoteService.WebSocketOpCode.ConnectionClose:
											// Strip mask bit and send unmasked close frame
											tabData[ulOffsetFrame + 1] = (byte)(tabData[ulOffsetFrame + 1] & 0b01111111);
											if(bMask) {
												for(ulong ul = 0; ul < ulLenData; ul++) {
													tabData[ulOffsetMask + ul] = tabData[ulOffsetData + ul];
												}
											}
											socClient.Send(tabData, (int)ulOffsetFrame, (int)(ulOffsetMask - ulOffsetFrame + ulLenData), System.Net.Sockets.SocketFlags.None);
											mreClientStop.Set();
											Service.Log("Client disconnected on socket [" + socClient.GetHashCode() + "]");
											break;
										case (byte)MagicRemoteService.WebSocketOpCode.Ping:
											tabData[ulOffsetFrame] = (byte)((tabData[ulOffsetFrame] & 0xF0) | (0x0A & 0x0F));
											tabData[ulOffsetFrame + 1] = (byte)(tabData[ulOffsetFrame + 1] & 0b01111111);
											if(bMask) {
												for(ulong ul = 0; ul < ulLenData; ul++) {
													tabData[ulOffsetMask + ul] = tabData[ulOffsetData + ul];
												}
											}
											socClient.Send(tabData, (int)ulOffsetFrame, (int)(ulOffsetMask - ulOffsetFrame + ulLenData), System.Net.Sockets.SocketFlags.None);
											Service.LogIfDebug("Ping received on socket [" + socClient.GetHashCode() + "]");
											break;
										case (byte)MagicRemoteService.WebSocketOpCode.Pong:
											if(ulLenData != 0) {
												switch(tabData[ulOffsetData + 0]) {
													case 0x01:
														tPongUserInput.Stop();
														using(System.Diagnostics.Process pProcessInact = new System.Diagnostics.Process()) {
															pProcessInact.StartInfo.FileName = "shutdown";
															pProcessInact.StartInfo.Arguments = "/s /t 300";
															pProcessInact.StartInfo.UseShellExecute = false;
															pProcessInact.StartInfo.WindowStyle = System.Diagnostics.ProcessWindowStyle.Hidden;
															pProcessInact.Start();
														}
														tUserInput.Start();
														Service.LogIfDebug("Pong inactivity received on socket [" + socClient.GetHashCode() + "]");
														break;
													default:
														Service.Warn("Unprocessed pong message [0x" + System.BitConverter.ToString(tabData, (int)ulOffsetData, (int)ulLenData).Replace("-", string.Empty) + "]");
														break;
												}
											} else {
												tPong.Stop();
												Service.LogIfDebug("Pong received on socket [" + socClient.GetHashCode() + "]");
											}
											break;
										default:
											Service.Warn("Unprocessed message [0x" + System.BitConverter.ToString(tabData, (int)ulOffsetData, (int)ulLenData).Replace("-", string.Empty) + "], " + System.Text.Encoding.Default.GetString(tabData, (int)ulOffsetData, (int)ulLenData));
											break;
									}
								}
								ulOffsetFrame = ulOffsetData + ulLenData;
							}

							if(!socClient.ReceiveAsync(eaClientReceiveAsync)) {
								ClientReceiveAsyncCompleted(socClient, eaClientReceiveAsync);
							}
							System.Threading.Thread.Sleep(1);
							break;
						default:
							throw new System.Exception("Unmanaged handle error");
					}
				}
				MagicRemoteService.SystemCursor.SetDefaultSystemCursor();
				MagicRemoteService.SystemCursor.SetDefaultMouseSpeedAccel();
				MagicRemoteService.Application.naehPowerSettingNotificationArrived -= PowerSettingNotificationArrived;
				tPing.Stop();
				tPing.Dispose();
				tPong.Stop();
				tPong.Dispose();
				tUserInput.Stop();
				tUserInput.Dispose();
				tInactivity.Stop();
				tInactivity.Dispose();
				tVideoInput.Stop();
				tVideoInput.Dispose();
				tPongUserInput.Stop();
				tPongUserInput.Dispose();
				mreClientStop.Dispose();
				eaClientReceiveAsync.Completed -= ClientReceiveAsyncCompleted;
				eaClientReceiveAsync.Dispose();
				areClientReceiveAsyncCompleted.Close();
				areClientReceiveAsyncCompleted.Dispose();
				socClient.Close();
				socClient.Dispose();
				System.Threading.Interlocked.Decrement(ref Service.iConnectedClients);
				Service.Log("Socket closed [" + socClient.GetHashCode() + "]");
			} catch(System.IO.IOException eException) {
				MagicRemoteService.SystemCursor.SetDefaultSystemCursor();
				MagicRemoteService.SystemCursor.SetDefaultMouseSpeedAccel();
				Service.Error("IO error in client thread: " + eException.ToString());
			} catch(System.ObjectDisposedException eException) {
				MagicRemoteService.SystemCursor.SetDefaultSystemCursor();
				MagicRemoteService.SystemCursor.SetDefaultMouseSpeedAccel();
				Service.Error("Object disposed in client thread: " + eException.ToString());
			} catch(System.Net.Sockets.SocketException eException) {
				MagicRemoteService.SystemCursor.SetDefaultSystemCursor();
				MagicRemoteService.SystemCursor.SetDefaultMouseSpeedAccel();
				Service.Error("Socket error in client thread: " + eException.ToString());
			} catch(System.InvalidOperationException eException) {
				MagicRemoteService.SystemCursor.SetDefaultSystemCursor();
				MagicRemoteService.SystemCursor.SetDefaultMouseSpeedAccel();
				Service.Error("Invalid operation in client thread: " + eException.ToString());
			}
		}
	}
	public static class PipeExtension {
		public static System.Diagnostics.Process GetServerProcess(this System.IO.Pipes.NamedPipeClientStream psClient) {
			WinApi.Kernel32.GetNamedPipeServerProcessId(psClient.SafePipeHandle.DangerousGetHandle(), out uint uiProcessId);
			return System.Diagnostics.Process.GetProcessById((int)uiProcessId);
		}
		public static System.Diagnostics.Process GetClientProcess(this System.IO.Pipes.NamedPipeServerStream psServer) {
			WinApi.Kernel32.GetNamedPipeClientProcessId(psServer.SafePipeHandle.DangerousGetHandle(), out uint uiProcessId);
			return System.Diagnostics.Process.GetProcessById((int)uiProcessId);
		}
	}
}
