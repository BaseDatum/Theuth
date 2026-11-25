package main

import "fmt"

func paint() {
	n := plugin.State.Int("n")
	plugin.Status(Status{Dot: "ok", Badge: n, Label: fmt.Sprint(n)})
	plugin.UI(UI{Text: fmt.Sprintf("%d presses", n), Buttons: []Button{{Label: "Press", Action: "inc"}}})
}
func main() {
	plugin.Action("inc", func(Action) any { plugin.State.Add("n", 1); paint(); return nil })
	plugin.Tool("presses", func(Call) any { return plugin.State.Int("n") })
	plugin.OnStart(paint)
	plugin.Run()
}
