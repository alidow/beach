fn main() {
    if cfg!(feature = "cabana_sck") {
        println!("cargo:warning=Feature `cabana_sck` is enabled, but the ScreenCaptureKit bridge is not yet implemented. See docs/beach-cabana/screencapturekit-spike.md.");
    }
}
