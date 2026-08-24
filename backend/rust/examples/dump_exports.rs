//! 临时工具（gitignored 可选）：枚举 PE DLL 的导出符号。
//! 用法：cargo run --example dump_exports -- <dll路径>
//! 用途：确认 backend/cuda/hdr_gpu_jni.dll 导出的是 JNI 还是 C ABI 符号。
use object::Object;

fn main() {
    let path = std::env::args().nth(1).expect("用法: dump_exports <dll>");
    let data = std::fs::read(&path).expect("读取失败");
    let file = object::File::parse(data.as_slice()).expect("解析 PE 失败");
    let exports = file.exports().expect("读取导出表失败");
    println!("导出符号数: {}", exports.len());
    for e in exports {
        println!("{}", String::from_utf8_lossy(e.name()));
    }
}