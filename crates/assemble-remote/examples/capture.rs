//! 本机手动探针：只采一帧并释放输入，不移动鼠标、不输入、不保存画面。
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut host = assemble_remote::native::Host::open()?;
    let frame = host.frame()?;
    host.release_all()?;
    println!(
        "capture_ok width={} height={} displays={} encoded_bytes={}",
        frame.width,
        frame.height,
        frame.displays.len(),
        frame.jpeg.len()
    );
    Ok(())
}
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn main() {
    eprintln!("remote desktop is supported on Windows and macOS");
    std::process::exit(1);
}
