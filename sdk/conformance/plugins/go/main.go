// Conformance fixture (not a template): every SDK implements this same behavior; see ../../host.py.
package main

import (
	"errors"
	"fmt"
)

func paint() {
	n := plugin.State.Int("n")
	plugin.Status(Status{Dot: "ok", Badge: n, Label: fmt.Sprint(n)})
	plugin.UI(UI{Text: fmt.Sprintf("%d presses", n), Buttons: []Button{{Label: "Press", Action: "inc"}}})
}

func main() {
	plugin.OnStart(func() {
		paint()
		r, err := plugin.Ping()
		if err != nil {
			panic(err)
		}
		plugin.Log("pong", map[string]any{"now_ms": r["now_ms"]})
	})
	plugin.OnRender(func(Action) { paint() }) // without it, render re-runs OnStart (which pings here)

	plugin.Action("inc", func(Action) any { plugin.State.Add("n", 1); paint(); return nil })
	plugin.Action("deadlock", func(Action) any {
		r, err := plugin.Request("ping", nil)
		if err != nil {
			return err
		}
		return r
	})
	plugin.Action("call", func(Action) any {
		r, err := plugin.Call("echo", map[string]any{"x": 1})
		if err != nil {
			return err
		}
		return r
	})
	plugin.Action("denied", func(Action) any {
		var e *RpcError
		if _, err := plugin.Call("shell", nil); errors.As(err, &e) {
			return map[string]any{"code": e.Code}
		}
		return nil
	})
	plugin.Action("boom", func(Action) any { return errors.New("boom") })
	plugin.Action("extras", func(req Action) any {
		plugin.Emit("pressed", map[string]any{"n": plugin.State.Int("n")})
		plugin.Notify("Hi", "there", "", nil)
		plugin.Invalidate()
		plugin.Send("custom/note", map[string]any{"a": 1})
		return req.Args
	})

	plugin.Tool("presses", func(Call) any { return fmt.Sprint(plugin.State.Int("n")) })
	plugin.Tool("fail", func(Call) any { panic("kaput") })

	plugin.Command("count", func(Action) any { return fmt.Sprintf("n=%d", plugin.State.Int("n")) })

	plugin.On("conformance.tick", func(ev Event) {
		inner, _ := ev.Data["data"].(map[string]any)
		plugin.Log("tick", map[string]any{"k": inner["k"]})
	})
	plugin.On("agent.events#TurnFinished", func(ev Event) {
		plugin.Log("turn", map[string]any{"stop_reason": ev.Data["stop_reason"]})
	})

	plugin.OnRequest("custom/echo", func(params map[string]any) any { return params })
	plugin.OnGranted(func(g map[string]any) {
		ps, _ := g["placements"].([]any)
		plugin.Log("granted", map[string]any{"placements": len(ps)})
	})
	plugin.OnShutdown(func() { plugin.Log("bye") })
	plugin.Run()
}
