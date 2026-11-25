// Conformance fixture (not a template): every SDK implements this same behavior; see ../../host.py.
mod theuth;
use theuth::*;

fn paint() {
    let n = plugin().state.int("n");
    plugin().status("ok", n, &n.to_string());
    plugin().ui(&format!("{n} presses"), &[("Press", "inc")]);
}

fn main() {
    let p = plugin();
    p.on_start(|| {
        paint();
        let r = plugin().ping().unwrap_or_default();
        plugin().log_with("pong", "info", json!({"now_ms": r["now_ms"]}));
    });
    p.on_render(|_| paint()); // without it, render re-runs on_start (which pings here)

    p.action("inc", |_| {
        plugin().state.add("n", 1);
        paint();
    });
    p.action("deadlock", |_| plugin().request("ping", json!({})));
    p.action("call", |_| plugin().call("echo", json!({"x": 1})));
    p.action("denied", |_| match plugin().call("shell", json!({})) {
        Err(e) => json!({"code": e.code}),
        Ok(_) => Value::Null,
    });
    p.action("boom", |_| -> Result<(), String> { Err("boom".into()) });
    p.action("extras", |req| {
        plugin().emit("pressed", json!({"n": plugin().state.int("n")}), None);
        plugin().notify("Hi", "there");
        plugin().invalidate(None);
        plugin().send("custom/note", json!({"a": 1}));
        req.args
    });

    p.tool("presses", |_| plugin().state.int("n").to_string());
    p.tool("fail", |_| -> String { panic!("kaput") });
    p.command("count", |_| format!("n={}", plugin().state.int("n")));

    p.on("conformance.tick", |ev| plugin().log_with("tick", "info", json!({"k": ev.data["data"]["k"]})));
    p.on("agent.events#TurnFinished", |ev| {
        plugin().log_with("turn", "info", json!({"stop_reason": ev.data["stop_reason"]}))
    });

    p.on_request("custom/echo", |params| params);
    p.on_granted(|g| {
        let n = g["placements"].as_array().map_or(0, Vec::len);
        plugin().log_with("granted", "info", json!({"placements": n}))
    });
    p.on_shutdown(|| plugin().log("bye"));
    p.run();
}
