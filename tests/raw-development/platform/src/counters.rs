//! Windows RAM/VRAM counters for the platform harness (lap-c0e / TASK-601).
//!
//! RAM: `GlobalMemoryStatusEx` (system totals) + `GetProcessMemoryInfo`
//! (working set and private commit of THIS process).
//! VRAM: DXGI `IDXGIAdapter3::QueryVideoMemoryInfo` on the first adapter
//! (local segment = discrete VRAM usage and budget), plus
//! `IDXGIAdapter1::GetDesc1` for the dedicated-video-memory size and adapter
//! description. Raw COM through `windows-sys` — no additional runtime
//! dependency.
//!
//! These are the counters recorded beside every benchmark workload so
//! measurements carry memory/resource context (spec A12). On non-Windows
//! targets every counter reports `unavailable` with an explicit reason: the
//! harness never invents a number.

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MemorySample {
    pub working_set_bytes: Option<u64>,
    pub private_commit_bytes: Option<u64>,
    pub total_phys_bytes: Option<u64>,
    pub avail_phys_bytes: Option<u64>,
    pub memory_load_percent: Option<u32>,
    pub vram_usage_bytes: Option<u64>,
    pub vram_budget_bytes: Option<u64>,
    pub vram_dedicated_bytes: Option<u64>,
    pub vram_adapter: Option<String>,
    pub vram_error: Option<String>,
}

impl MemorySample {
    /// Compact series entry for the results document.
    pub fn series_entry(&self, label: &str) -> serde_json::Value {
        serde_json::json!({
            "label": label,
            "workingSetBytes": self.working_set_bytes,
            "privateCommitBytes": self.private_commit_bytes,
            "availPhysBytes": self.avail_phys_bytes,
            "vramUsageBytes": self.vram_usage_bytes,
            "vramBudgetBytes": self.vram_budget_bytes,
        })
    }
}

#[cfg(windows)]
mod imp {
    use super::MemorySample;
    use std::ffi::c_void;

    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // -------------------------------------------------------------------------
    // Minimal raw-COM DXGI surface (windows-sys does not ship the COM-heavy
    // DXGI API; these declarations link against the Windows SDK's dxgi.lib).
    // -------------------------------------------------------------------------

    /// `[repr(C)]` mirrors the Win32 `GUID` layout exactly (16 bytes).
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    // {770AAE78-F26F-4DBA-A829-253C83D1B387}
    const IID_IDXGIFACTORY1: Guid = Guid {
        data1: 0x770aae78,
        data2: 0xf26f,
        data3: 0x4dba,
        data4: [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
    };
    // {645967A4-1392-4310-A798-8053CE3E93FD} (dxgi1_4.h, IID_IDXGIAdapter3)
    const IID_IDXGIADAPTER3: Guid = Guid {
        data1: 0x645967a4,
        data2: 0x1392,
        data3: 0x4310,
        data4: [0xa7, 0x98, 0x80, 0x53, 0xce, 0x3e, 0x93, 0xfd],
    };

    /// Layout mirrors `DXGI_ADAPTER_DESC1` from the Windows SDK exactly
    /// (Description, VendorId, DeviceId, SubSysId, Revision, three SIZE_T
    /// memory sizes, LUID, Flags; 8-byte alignment, 312 bytes total).
    #[repr(C)]
    struct DxgiAdapterDesc1 {
        description: [u16; 128],
        vendor_id: u32,
        device_id: u32,
        sub_sys_id: u32,
        revision: u32,
        dedicated_video_memory: usize,
        dedicated_system_memory: usize,
        shared_system_memory: usize,
        luid_low_part: u32,
        luid_high_part: i32,
        flags: u32,
    }

    #[repr(C)]
    struct DxgiQueryVideoMemoryInfo {
        budget: u64,
        current_usage: u64,
        available_for_reservation: u64,
        current_reservation: u64,
    }

    const DXGI_MEMORY_SEGMENT_GROUP_LOCAL: i32 = 1;

    #[link(name = "dxgi")]
    unsafe extern "system" {
        fn CreateDXGIFactory1(riid: *const Guid, ppfactory: *mut *mut c_void) -> i32;
    }

    type QueryInterfaceFn =
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32;
    type ReleaseFn = unsafe extern "system" fn(*mut c_void) -> u32;
    type EnumAdapters1Fn = unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32;
    type GetDesc1Fn = unsafe extern "system" fn(*mut c_void, *mut DxgiAdapterDesc1) -> i32;
    type QueryVideoMemoryInfoFn =
        unsafe extern "system" fn(*mut c_void, u32, i32, *mut DxgiQueryVideoMemoryInfo) -> i32;

    unsafe fn vtable_method(obj: *mut c_void, index: usize) -> *const c_void {
        // SAFETY: `obj` is a live COM object pointer and `index` addresses a
        // slot of its vtable.
        unsafe {
            let vtbl = *(obj as *mut *mut c_void);
            *(vtbl as *const *const c_void).add(index)
        }
    }

    unsafe fn release(obj: *mut c_void) {
        if obj.is_null() {
            return;
        }
        // SAFETY: `obj` is a live COM object; slot 2 is IUnknown::Release.
        unsafe {
            let release: ReleaseFn = std::mem::transmute(vtable_method(obj, 2));
            release(obj);
        }
    }

    /// VRAM (local segment) usage/budget of the first DXGI adapter plus its
    /// dedicated-memory size and description.
    unsafe fn query_vram() -> Result<(Option<u64>, Option<u64>, Option<u64>, Option<String>), String>
    {
        let mut factory: *mut c_void = std::ptr::null_mut();
        let hr = CreateDXGIFactory1(&IID_IDXGIFACTORY1, &mut factory);
        if hr != 0 {
            return Err(format!("CreateDXGIFactory1 failed with HRESULT 0x{hr:08x}"));
        }
        let result = (|| {
            let enum_adapters1: EnumAdapters1Fn = std::mem::transmute(vtable_method(factory, 12));
            let mut adapter1: *mut c_void = std::ptr::null_mut();
            let hr = enum_adapters1(factory, 0, &mut adapter1);
            if hr != 0 {
                return Err(format!("EnumAdapters1(0) failed with HRESULT 0x{hr:08x}"));
            }

            let mut description = None;
            let mut dedicated = None;
            let get_desc1: GetDesc1Fn = std::mem::transmute(vtable_method(adapter1, 10));
            let mut desc: DxgiAdapterDesc1 = std::mem::zeroed();
            let hr = get_desc1(adapter1, &mut desc);
            if hr == 0 {
                let len = desc
                    .description
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(desc.description.len());
                description = Some(
                    String::from_utf16_lossy(&desc.description[..len])
                        .trim()
                        .to_string(),
                );
                dedicated = Some(desc.dedicated_video_memory as u64);
            } else {
                return Err(format!("GetDesc1 failed with HRESULT 0x{hr:08x}"));
            }

            let mut usage = None;
            let mut budget = None;
            let mut adapter3: *mut c_void = std::ptr::null_mut();
            let qi: QueryInterfaceFn = std::mem::transmute(vtable_method(adapter1, 0));
            let hr_qi = qi(adapter1, &IID_IDXGIADAPTER3, &mut adapter3);
            if hr_qi == 0 {
                let query: QueryVideoMemoryInfoFn =
                    std::mem::transmute(vtable_method(adapter3, 14));
                let mut info: DxgiQueryVideoMemoryInfo = std::mem::zeroed();
                let hr_query = query(adapter3, 0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info);
                if hr_query == 0 {
                    usage = Some(info.current_usage);
                    budget = Some(info.budget);
                } else {
                    release(adapter3);
                    return Err(format!(
                        "QueryVideoMemoryInfo failed with HRESULT 0x{hr_query:08x}"
                    ));
                }
                release(adapter3);
            } else {
                return Err(format!(
                    "QueryInterface(IDXGIAdapter3) failed with HRESULT 0x{hr_qi:08x}"
                ));
            }

            release(adapter1);
            Ok((usage, budget, dedicated, description))
        })();

        release(factory);
        result
    }

    pub fn sample() -> MemorySample {
        let mut out = MemorySample::default();

        let mut memstat: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        memstat.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut memstat) } != 0 {
            out.total_phys_bytes = Some(memstat.ullTotalPhys);
            out.avail_phys_bytes = Some(memstat.ullAvailPhys);
            out.memory_load_percent = Some(memstat.dwMemoryLoad);
        }

        let mut pmc: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let ok = unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut pmc as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS,
                pmc.cb,
            )
        };
        if ok != 0 {
            out.working_set_bytes = Some(pmc.WorkingSetSize as u64);
            out.private_commit_bytes = Some(pmc.PrivateUsage as u64);
        }

        match unsafe { query_vram() } {
            Ok((usage, budget, dedicated, description)) => {
                out.vram_usage_bytes = usage;
                out.vram_budget_bytes = budget;
                out.vram_dedicated_bytes = dedicated;
                out.vram_adapter = description;
            }
            Err(error) => {
                out.vram_error = Some(error);
            }
        }

        out
    }
}

#[cfg(windows)]
pub use imp::sample;

#[cfg(not(windows))]
pub fn sample() -> MemorySample {
    MemorySample {
        vram_error: Some(
            "memory/VRAM counters are implemented for Windows only; this platform's sample is explicitly unavailable".to_string(),
        ),
        ..MemorySample::default()
    }
}
