//! Re-encode a decoded image through the upstream openjp2 port using the
//! jpeg2k wrapper's own (file-based) encode path, to check the port encoder.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let data = std::fs::read(&a[1]).unwrap();
    let img = jpeg2k::Image::from_bytes(&data).unwrap();
    println!("decoded {}x{} comps={}", img.width(), img.height(), img.num_components());
    img.save_as_file_with(&a[2], jpeg2k::EncodeParameters::default()).unwrap();
    let back = std::fs::read(&a[2]).unwrap();
    println!("re-encoded {} bytes", back.len());
    let again = jpeg2k::Image::from_bytes(&back).unwrap();
    println!("re-decoded {}x{}", again.width(), again.height());
}
