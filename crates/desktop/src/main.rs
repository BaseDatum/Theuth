//! Desktop binary: the app lives in `desktop_lib` so the iOS bridge can link
//! it as a static library (31 §8.3).

fn main() {
    desktop_lib::run()
}
