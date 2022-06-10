fn main() {
    println!("cargo:rustc-link-search=/opt/xen-arm/dist/install/usr/lib/");
    println!("cargo:rustc-link-lib=xenstore");
    println!("cargo:rustc-link-lib=xendevicemodel");
    println!("cargo:rustc-link-lib=xentoolcore");
    println!("cargo:rustc-link-lib=xentoollog");
    println!("cargo:rustc-link-lib=xencall");
    println!("cargo:rustc-link-lib=xenctrl");
}
