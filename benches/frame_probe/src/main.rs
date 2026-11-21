//! Times DataDiver frames from posted input to the screen.
//!
//! Finds the DataDiver window of `--pid`, posts wheel or key input to it at a
//! fixed rate, and watches the screen with Desktop Duplication. The app must be
//! built with the frame-trace switch (`DD_FRAME_TRACE`, from
//! `performance/frame-experiments.patch`): it draws each frame's number as an
//! 18-cell marker in the client's top-left corner and writes per-frame QPC
//! timestamps on exit. Desktop Duplication reports when each marker reached the
//! screen, so every input gets three latencies: posted → dispatched, posted →
//! frame submitted (EndDraw/Present returned), and posted → on screen.
//!
//! Prints one JSON line with medians and percentiles; `--out` also writes the
//! per-input rows as CSV.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Performance::*;
use windows::Win32::System::Threading::*;
use windows::Win32::System::WindowsProgramming::QueryProcessCycleTime;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_DOWN;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, Interface, Result};

const CELL: i32 = 6;
const CELLS: i32 = 18;

struct Args {
    pid: u32,
    seconds: f64,
    rate: f64,
    input: String,
    trace: String,
    out: Option<String>,
    label: String,
}

fn args() -> Args {
    let mut args = Args {
        pid: 0,
        seconds: 5.0,
        rate: 120.0,
        input: "wheel".into(),
        trace: String::new(),
        out: None,
        label: "run".into(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let value = it.next().unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--pid" => args.pid = value.parse().expect("--pid"),
            "--seconds" => args.seconds = value.parse().expect("--seconds"),
            "--rate" => args.rate = value.parse().expect("--rate"),
            "--input" => args.input = value,
            "--trace" => args.trace = value,
            "--out" => args.out = Some(value),
            "--label" => args.label = value,
            _ => panic!("unknown flag {flag}"),
        }
    }
    assert!(args.pid != 0 && !args.trace.is_empty(), "usage: frame_probe --pid N --trace PATH [--seconds S --rate HZ --input wheel|key --out CSV --label L]");
    args
}

fn qpc() -> i64 {
    let mut value = 0;
    unsafe {
        let _ = QueryPerformanceCounter(&mut value);
    }
    value
}

fn frequency() -> f64 {
    let mut value = 0;
    unsafe {
        let _ = QueryPerformanceFrequency(&mut value);
    }
    value as f64
}

fn find_window(pid: u32) -> Option<HWND> {
    struct Search {
        pid: u32,
        found: Option<HWND>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        let mut class = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
        if pid == search.pid && unsafe { IsWindowVisible(hwnd) }.as_bool() && String::from_utf16_lossy(&class[..len]) == "DataDiverWindow" {
            search.found = Some(hwnd);
            return false.into();
        }
        true.into()
    }
    let mut search = Search { pid, found: None };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
    }
    search.found
}

struct Duplicator {
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
    /// The marker's top-left, relative to the output.
    origin: (i32, i32),
}

impl Duplicator {
    fn new(screen: POINT) -> Result<Self> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut index = 0;
            loop {
                let adapter = factory.EnumAdapters1(index)?;
                index += 1;
                let mut output_index = 0;
                while let Ok(output) = adapter.EnumOutputs(output_index) {
                    output_index += 1;
                    let desc = output.GetDesc()?;
                    let r = desc.DesktopCoordinates;
                    if screen.x < r.left || screen.x >= r.right || screen.y < r.top || screen.y >= r.bottom {
                        continue;
                    }
                    let mut device = None;
                    let mut context = None;
                    D3D11CreateDevice(
                        &adapter,
                        D3D_DRIVER_TYPE_UNKNOWN,
                        HMODULE::default(),
                        D3D11_CREATE_DEVICE_FLAG(0),
                        None,
                        D3D11_SDK_VERSION,
                        Some(&mut device),
                        None,
                        Some(&mut context),
                    )?;
                    let device = device.unwrap();
                    let output1: IDXGIOutput1 = output.cast()?;
                    let duplication = output1.DuplicateOutput(&device)?;
                    let desc = D3D11_TEXTURE2D_DESC {
                        Width: (CELL * CELLS) as u32,
                        Height: CELL as u32,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        Usage: D3D11_USAGE_STAGING,
                        BindFlags: 0,
                        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                        MiscFlags: 0,
                    };
                    let mut staging = None;
                    device.CreateTexture2D(&desc, None, Some(&mut staging))?;
                    return Ok(Self {
                        context: context.unwrap(),
                        duplication,
                        staging: staging.unwrap(),
                        origin: (screen.x - r.left, screen.y - r.top),
                    });
                }
            }
        }
    }

    /// The next desktop update's present time and the marker on it, if any.
    fn next(&self, timeout_ms: u32) -> Result<Option<(i64, Option<u32>)>> {
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource = None;
            match self.duplication.AcquireNextFrame(timeout_ms, &mut info, &mut resource) {
                Ok(()) => {}
                Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
                Err(error) => return Err(error),
            }
            let mut result = None;
            if info.LastPresentTime != 0 {
                let texture: ID3D11Texture2D = resource.unwrap().cast()?;
                let (x, y) = self.origin;
                let region = D3D11_BOX {
                    left: x as u32,
                    top: y as u32,
                    front: 0,
                    right: (x + CELL * CELLS) as u32,
                    bottom: (y + CELL) as u32,
                    back: 1,
                };
                self.context.CopySubresourceRegion(&self.staging, 0, 0, 0, 0, &texture, 0, Some(&region));
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                self.context.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
                let pixels = mapped.pData as *const u8;
                let bit = |cell: i32| {
                    let offset = (CELL / 2) as usize * mapped.RowPitch as usize + ((cell * CELL + CELL / 2) * 4) as usize;
                    let p = std::slice::from_raw_parts(pixels.add(offset), 3);
                    (p[0] as u32 + p[1] as u32 + p[2] as u32) > 384
                };
                let marker = if bit(0) && !bit(CELLS - 1) {
                    Some((1..=16).fold(0u32, |n, cell| (n << 1) | bit(cell) as u32))
                } else {
                    None
                };
                self.context.Unmap(&self.staging, 0);
                result = Some((info.LastPresentTime, marker));
            }
            self.duplication.ReleaseFrame()?;
            Ok(result.or(Some((0, None))))
        }
    }
}

fn cycles(process: HANDLE) -> u64 {
    let mut value = 0;
    unsafe {
        let _ = QueryProcessCycleTime(process, &mut value);
    }
    value
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let rank = ((values.len() - 1) as f64 * p).round() as usize;
    values[rank]
}

struct Frame {
    number: u32,
    inputs: u32,
    wait: i64,
    begin: i64,
    drawn: i64,
    ended: i64,
    presented: i64,
}

fn main() -> Result<()> {
    let args = args();
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let freq = frequency();
    let ms = |ticks: i64| ticks as f64 * 1000.0 / freq;

    let start = Instant::now();
    let hwnd = loop {
        if let Some(hwnd) = find_window(args.pid) {
            break hwnd;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "no DataDiver window for pid {}", args.pid);
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut origin = POINT::default();
    let mut client = RECT::default();
    unsafe {
        // The marker must not be covered, so keep the window on top.
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        std::thread::sleep(Duration::from_millis(300));
        let _ = ClientToScreen(hwnd, &mut origin);
        let _ = GetClientRect(hwnd, &mut client);
    }
    let center = POINT {
        x: origin.x + client.right / 2,
        y: origin.y + client.bottom / 2,
    };
    let duplicator = Duplicator::new(origin)?;
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, false, args.pid)? };

    // Drain the first full-desktop frame.
    while duplicator.next(50)?.is_some() {}

    let cycles_before = cycles(process);
    let posts_wanted = (args.seconds * args.rate) as usize;
    let period = (freq / args.rate) as i64;
    let handle = hwnd.0 as isize;
    let input = args.input.clone();
    let poster = std::thread::spawn(move || {
        let hwnd = HWND(handle as _);
        let (message, wparam, lparam) = match input.as_str() {
            "key" => (WM_KEYDOWN, WPARAM(VK_DOWN.0 as usize), LPARAM(1 | (0x50 << 16) | (1 << 24))),
            _ => (
                WM_MOUSEWHEEL,
                WPARAM(((-120i16 as u16 as usize) << 16) as usize),
                LPARAM(((center.y as u16 as isize) << 16) | (center.x as u16 as isize)),
            ),
        };
        unsafe {
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        }
        let t0 = qpc() + period;
        let mut posts = Vec::with_capacity(posts_wanted);
        for i in 0..posts_wanted {
            let due = t0 + i as i64 * period;
            loop {
                let left = due - qpc();
                if left <= 0 {
                    break;
                }
                if left as f64 / freq > 0.0015 {
                    std::thread::sleep(Duration::from_micros(500));
                } else {
                    std::hint::spin_loop();
                }
            }
            posts.push(qpc());
            unsafe {
                let _ = PostMessageW(Some(hwnd), message, wparam, lparam);
                if message == WM_KEYDOWN {
                    let _ = PostMessageW(Some(hwnd), WM_KEYUP, wparam, LPARAM(lparam.0 | (3 << 30)));
                }
            }
        }
        posts
    });

    let mut shown: Vec<(i64, Option<u32>)> = Vec::new();
    while !poster.is_finished() {
        if let Some(update) = duplicator.next(20)? {
            shown.push(update);
        }
    }
    let posts = poster.join().unwrap();
    let tail = Instant::now();
    while tail.elapsed() < Duration::from_millis(400) {
        if let Some(update) = duplicator.next(20)? {
            shown.push(update);
        }
    }
    let cycles_after = cycles(process);
    let input_span = (posts.last().unwrap() - posts[0]) as f64 / freq;

    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        WaitForSingleObject(process, 15_000);
    }
    let trace = std::fs::read_to_string(&args.trace).expect("read the app's frame trace");
    let mut frames = Vec::new();
    let mut dispatched = Vec::new();
    for line in trace.lines() {
        let f: Vec<&str> = line.split(',').collect();
        match f[0] {
            "I" => dispatched.push(f[2].parse::<i64>().unwrap()),
            "F" => {
                let n = |i: usize| f[i].parse::<i64>().unwrap();
                frames.push(Frame {
                    number: n(1) as u32,
                    inputs: n(2) as u32,
                    wait: n(3),
                    begin: n(4),
                    drawn: n(5),
                    ended: n(6),
                    presented: n(7),
                })
            }
            _ => {}
        }
    }
    // Only inputs after the probe started posting count; the app may have
    // seen none before, but keep the offset honest.
    let first_input = dispatched.iter().position(|&t| t >= posts[0]).unwrap_or(0);
    let posted_inputs = dispatched.len() - first_input;

    // The first time each frame number reached the screen.
    let mut on_screen: Vec<(u32, i64)> = Vec::new();
    let mut unwrapped: Option<u32> = None;
    let mut bad_markers = 0;
    for &(time, marker) in &shown {
        let Some(value) = marker else {
            if time != 0 {
                bad_markers += 1;
            }
            continue;
        };
        let full = match unwrapped {
            None => frames.iter().map(|f| f.number).filter(|n| n & 0xffff == value).max().unwrap_or(value),
            Some(last) => last + (value.wrapping_sub(last) & 0xffff),
        };
        if unwrapped != Some(full) {
            on_screen.push((full, time));
            unwrapped = Some(full);
        }
    }

    let mut rows = Vec::new();
    let (mut to_dispatch, mut to_submit, mut to_screen) = (Vec::new(), Vec::new(), Vec::new());
    let mut missing = 0;
    for (i, &posted) in posts.iter().enumerate() {
        let index = first_input + i;
        let Some(&dispatch) = dispatched.get(index) else {
            missing += 1;
            continue;
        };
        let Some(frame) = frames.iter().find(|f| f.inputs as usize > index) else {
            missing += 1;
            continue;
        };
        let Some(&(_, displayed)) = on_screen.iter().find(|(n, _)| *n >= frame.number) else {
            missing += 1;
            continue;
        };
        to_dispatch.push(ms(dispatch - posted));
        to_submit.push(ms(frame.presented - posted));
        to_screen.push(ms(displayed - posted));
        rows.push(format!("{i},{},{},{},{},{}", frame.number, ms(dispatch - posted), ms(frame.begin - posted), ms(frame.presented - posted), ms(displayed - posted)));
    }

    let window: Vec<&Frame> = frames.iter().filter(|f| f.begin >= posts[0] && f.begin <= *posts.last().unwrap()).collect();
    let mut paint: Vec<f64> = window.iter().map(|f| ms(f.drawn - f.begin)).collect();
    let mut end_draw: Vec<f64> = window.iter().map(|f| ms(f.ended - f.drawn)).collect();
    let mut present: Vec<f64> = window.iter().map(|f| ms(f.presented - f.ended)).collect();
    let mut wait: Vec<f64> = window.iter().map(|f| ms(f.begin - f.wait)).collect();
    let mut interval: Vec<f64> = window.windows(2).map(|w| ms(w[1].begin - w[0].begin)).collect();
    let shown_in_window = on_screen.iter().filter(|(_, t)| *t >= posts[0] && *t <= *posts.last().unwrap()).count();

    if let Some(path) = &args.out {
        let mut text = String::from("input,frame,dispatch_ms,frame_begin_ms,submit_ms,screen_ms\n");
        text.push_str(&rows.join("\n"));
        std::fs::write(path, text).expect("write --out");
    }
    let stat = |values: &mut Vec<f64>| {
        format!(
            "{{\"p50\":{:.2},\"p95\":{:.2},\"p99\":{:.2},\"max\":{:.2}}}",
            percentile(values, 0.5),
            percentile(values, 0.95),
            percentile(values, 0.99),
            percentile(values, 1.0)
        )
    };
    println!(
        "{{\"label\":\"{}\",\"input\":\"{}\",\"rate\":{},\"posts\":{},\"dispatched\":{},\"matched\":{},\"missing\":{},\"bad_markers\":{},\
\"client\":[{},{}],\"frames_per_s\":{:.1},\"shown_per_s\":{:.1},\"mcycles_per_s\":{:.1},\
\"to_dispatch\":{},\"to_submit\":{},\"to_screen\":{},\"paint\":{},\"end_draw\":{},\"present\":{},\"wait\":{},\"interval\":{}}}",
        args.label,
        args.input,
        args.rate,
        posts.len(),
        posted_inputs,
        to_screen.len(),
        missing,
        bad_markers,
        client.right,
        client.bottom,
        window.len() as f64 / input_span,
        shown_in_window as f64 / input_span,
        (cycles_after - cycles_before) as f64 / 1e6 / input_span,
        stat(&mut to_dispatch),
        stat(&mut to_submit),
        stat(&mut to_screen),
        stat(&mut paint),
        stat(&mut end_draw),
        stat(&mut present),
        stat(&mut wait),
        stat(&mut interval),
    );
    Ok(())
}
