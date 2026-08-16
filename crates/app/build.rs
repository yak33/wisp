//! 把应用图标嵌进 exe 的资源段。图标源与托盘图标同出
//! tools/gen-icons（docs/icon.svg + docs/favicon.svg）。

fn main() {
    if cfg!(target_os = "windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/icons/app.ico");
        resource.compile().expect("嵌入应用图标失败");
    }
    println!("cargo:rerun-if-changed=assets/icons/app.ico");
}
