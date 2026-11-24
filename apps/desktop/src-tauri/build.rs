// 构建时读取服务端地址。环境变量变了就重新编译，不用手动清理。
fn main() {
    println!("cargo::rerun-if-env-changed=ASSEMBLE_SERVER_ORIGIN");
    tauri_build::build();

    // Windows 上把应用清单也链进测试程序。
    //
    // tauri_build 只把清单链进主程序，而清单里声明了对通用控件库 6.0 的依赖。库的单元测试是
    // 另一个可执行文件：没有清单，系统给它的是 5.82 的旧版，运行时的消息对话框要用的
    // TaskDialogIndirect 只在 6.0 里有，于是测试程序一启动就以 STATUS_ENTRYPOINT_NOT_FOUND
    // 退出，一个测试都没跑。这是持续集成的 Windows 一栏抓到的（2026-11-24）。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let resource = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default())
            .join("resource.rc");
        embed_resource::compile_for_tests(resource, embed_resource::NONE)
            .manifest_required()
            .expect("the application manifest could not be linked into the tests");
    }
}
