mod theuth;
use theuth::*;
fn paint() {
    let n = plugin().state.int("n");
    plugin().status("ok", n, &n.to_string());
    plugin().ui(&format!("{n} presses"), &[("Press", "inc")]);
}
fn main() {
    plugin().action("inc", |_| { plugin().state.add("n", 1); paint(); });
    plugin().tool("presses", |_| plugin().state.int("n").to_string());
    plugin().on_start(paint);
    plugin().run();
}
