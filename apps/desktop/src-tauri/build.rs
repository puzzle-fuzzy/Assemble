// 构建时读取服务端地址。环境变量变了就重新编译，不用手动清理。
fn main() {
    println!("cargo::rerun-if-env-changed=ASSEMBLE_SERVER_ORIGIN");
    tauri_build::build();
}
