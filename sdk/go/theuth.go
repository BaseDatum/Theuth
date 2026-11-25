// theuth plugin SDK for Go 1.22+ v0.1.0, stdlib only (design 216). Vendored: copy theuth.go beside main.go.

// A theuth plugin in a few lines. theuth.go is package main; your main.go sits beside it:
//
//	func main() {
//		plugin.Action("inc", func(Action) any { plugin.State.Add("n", 1); return nil })
//		plugin.Run()
//	}
//
// plugin.yaml is the contract: the host learns tools, subscriptions and placements from it, never by
// running this code. `go run . --theuth-manifest` prints what the code registered, to compare.
//
// Stdout is the protocol. fmt.Println and other os.Stdout writes corrupt it (and a flood gets the plugin
// killed): use plugin.Log. Stderr is the log file. Handlers run one at a time on the goroutine that called
// Run; a reader goroutine routes responses, so Request/Call from a handler never deadlocks, and no SDK
// lock is held while a handler runs. Raw hatch: Send, Request, OnRequest, OnNotify.
//
// Handlers return any JSON value. A returned *RpcError answers with that error; any other error, or a
// panic, answers -32603 (tools: an isError result) and is logged. The process stays up. Notification
// verbs panic on a local error (no single default placement, params not JSON-encodable).
package main

import (
	"bufio"
	"bytes"
	"cmp"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"sync"
)

// PROTOCOL is the wire protocol version this SDK speaks.
const PROTOCOL = 1

// plugin is the singleton: register handlers on it, then call plugin.Run().
var plugin = newPlugin()

// RpcError is a JSON-RPC error. Return it from a handler to answer with it; Request and Call return it.
type RpcError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
	Data    any    `json:"data,omitempty"`
}

func (e *RpcError) Error() string { return e.Message }

// Request is what action, query, command, render and tool handlers get. Args is params.args (tools:
// params.arguments); Session falls back to params._meta.theuth.session; Raw is the whole message.
type Request struct {
	Placement, Name, Session, Client, Activation string
	Args, Form, ID                               any
	Raw                                          map[string]any
}

// Action is the request an action, query, command or render handler gets.
type Action = Request

// Call is the request a tool handler gets.
type Call = Request

// Event is a subscribed event. Data is the JSON object the host sent, unfiltered.
type Event struct {
	Topic, Type, Session string
	Version, AtMs        int64
	Data, Raw            map[string]any
}

// Status is a placement's dot (ok|busy|warn|error|off|none). Badge: an int (0 is sent and hides it) or
// ≤4 chars; nil omits it. Empty Label/Tooltip are omitted; empty Placement = the only granted one.
type Status struct {
	Dot                       string
	Badge                     any
	Label, Tooltip, Placement string
}

// Button is one OpenUI button; Style "" means "secondary".
type Button struct{ Label, Action, Style string }

// UI replaces a placement's content. Text is the TUI/channel fallback and always sent. Without Source,
// format openui (or "") builds an OpenUI card from Text and Buttons; any other format sends Text as source.
type UI struct {
	Text                      string
	Buttons                   []Button
	Source, Format, Placement string
}

type registry[F any] struct {
	names []string // registration order, for the manifest
	fns   map[string]F
}

func (r *registry[F]) put(name string, fn F) {
	if r.fns == nil {
		r.fns = map[string]F{}
	}
	if _, ok := r.fns[name]; !ok {
		r.names = append(r.names, name)
	}
	r.fns[name] = fn
}

type reply struct {
	result any
	err    error
}

// Plugin is the SDK state. Hello, Grants, DataDir and Clients are set from `hello`/`granted` on the
// handler goroutine; read them from handlers.
type Plugin struct {
	State                             *State
	Hello, Grants, Clients            map[string]any
	DataDir                           string
	events                            registry[func(Event)]
	actions, queries, commands, tools registry[func(Request) any]
	requests                          registry[func(map[string]any) any]
	notifies                          registry[func(map[string]any)]
	start, shutdown                   []func()
	granted, lagged, cancel           []func(map[string]any)
	render                            []func(Request)
	mu                                sync.Mutex // guards pending, nextID, cancelled, eof
	pending                           map[string]chan reply
	nextID                            int
	cancelled                         map[string]bool
	eof                               bool
	outMu                             sync.Mutex
	sink                              func(map[string]any)           // Drive mode
	responder                         func(string, any) (any, error) // Drive mode
}

func newPlugin() *Plugin {
	p := &Plugin{Hello: map[string]any{}, Grants: map[string]any{}, Clients: map[string]any{},
		pending: map[string]chan reply{}, cancelled: map[string]bool{}}
	p.State = &State{owner: p}
	return p
}

// On handles an event. name: "agent.events#TurnFinished", "agent.events", "turns.limit_reached" or "*".
// Lookup order is topic#type, topic, type, "*"; the first match runs.
func (p *Plugin) On(name string, fn func(Event)) { p.events.put(name, fn) }

// Action handles an `action` request. nil -> {"ok": true}.
func (p *Plugin) Action(name string, fn func(Action) any) { p.actions.put(name, fn) }

// Query handles an OpenUI Query(name, …). The return value is the query's value.
func (p *Plugin) Query(name string, fn func(Action) any) { p.queries.put(name, fn) }

// Command handles `/plugin:name`. A string -> {"text": s}, an object as-is, nil -> {}.
func (p *Plugin) Command(name string, fn func(Action) any) { p.commands.put(name, fn) }

// Tool handles `tools/call` for a tool declared in plugin.yaml. A string -> text content; an object
// with content/isError as-is; any other value -> its compact JSON as text (+ structuredContent for objects).
func (p *Plugin) Tool(name string, fn func(Call) any) { p.tools.put(name, fn) }

// OnRequest is the raw hatch for a host request the helpers do not wrap: fn(params) -> result.
func (p *Plugin) OnRequest(method string, fn func(map[string]any) any) { p.requests.put(method, fn) }

// OnNotify is the raw hatch for a host notification: fn(params).
func (p *Plugin) OnNotify(method string, fn func(map[string]any)) { p.notifies.put(method, fn) }

// OnStart runs after `hello`, before the loop. Paint here. Re-run on `render` unless OnRender is set.
// Hooks may be registered several times and run in order; a panic in one is logged, the process stays up.
func (p *Plugin) OnStart(fn func()) { p.start = append(p.start, fn) }

// OnShutdown runs on `shutdown` or stdin EOF, before exit 0.
func (p *Plugin) OnShutdown(fn func()) { p.shutdown = append(p.shutdown, fn) }

// OnGranted runs with the new grants when they change; plugin.Grants is already replaced.
func (p *Plugin) OnGranted(fn func(grants map[string]any)) { p.granted = append(p.granted, fn) }

// OnLagged runs when events were dropped ({dropped, since_ms}). Default without hooks: a warn log.
func (p *Plugin) OnLagged(fn func(params map[string]any)) { p.lagged = append(p.lagged, fn) }

// OnCancel runs on notifications/cancelled ({requestId, reason}); Cancelled(id) is already true.
func (p *Plugin) OnCancel(fn func(params map[string]any)) { p.cancel = append(p.cancel, fn) }

// OnRender handles a `render` request (Placement, Session, Client). Send a UI; the SDK replies {}.
func (p *Plugin) OnRender(fn func(Action)) { p.render = append(p.render, fn) }

// Cancelled reports whether the host cancelled its request id. The handler is not stopped; a late
// reply is still sent and the host discards it.
func (p *Plugin) Cancelled(id any) bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.cancelled[fmt.Sprint(id)]
}

// Send is a raw notification; nil params -> {}.
func (p *Plugin) Send(method string, params any) {
	if err := p.write(map[string]any{"jsonrpc": "2.0", "method": method, "params": orEmpty(params)}); err != nil {
		panic(err)
	}
}

// at sets m["placement"]: given, else the single id in grants.placements.
func (p *Plugin) at(given string, m map[string]any) map[string]any {
	if given == "" {
		ids, _ := p.Grants["placements"].([]any)
		if len(ids) != 1 {
			panic(fmt.Errorf("set Placement: the grant holds %d placements %v", len(ids), ids))
		}
		given = str(ids[0])
	}
	m["placement"] = given
	return m
}

// Status sets the placement's dot, badge, label and tooltip.
func (p *Plugin) Status(s Status) {
	m := p.at(s.Placement, map[string]any{"dot": s.Dot})
	for k, v := range map[string]any{"badge": s.Badge, "label": s.Label, "tooltip": s.Tooltip} {
		if v != nil && v != "" {
			m[k] = v
		}
	}
	p.Send("status", m)
}

// UI replaces the placement's content; see type UI.
func (p *Plugin) UI(u UI) {
	if u.Source == "" && (u.Format == "" || u.Format == "openui") {
		u.Source = OpenUICard(u.Text, u.Buttons)
	} else if u.Source == "" {
		u.Source = u.Text
	}
	u.Format = cmp.Or(u.Format, "openui")
	p.Send("ui", p.at(u.Placement, map[string]any{"format": u.Format, "text": u.Text, "source": u.Source}))
}

// OpenUICard is the OpenUI source UI builds from text and buttons: deterministic, so equal input never remounts.
func OpenUICard(text string, buttons []Button) string {
	lines, calls := []string{"root = Card([body])", "body = TextContent(" + jsStr(text) + ")"}, []string{}
	for i, b := range buttons {
		lines = append(lines, fmt.Sprintf("a%d = Mutation(%s, {})", i, jsStr(b.Action)))
		calls = append(calls, fmt.Sprintf("Button(%s, @Run(a%d), %s)", jsStr(b.Label), i, jsStr(cmp.Or(b.Style, "secondary"))))
	}
	if len(buttons) > 0 {
		lines[0] = "root = Card([body, controls])"
		lines = append(lines, "controls = Buttons(["+strings.Join(calls, ", ")+"])")
	}
	return strings.Join(lines, "\n")
}

// Emit publishes a declared event. An undeclared name is dropped by the host (-32003 in the plugin
// log); Emit cannot see that, because it is a notification.
func (p *Plugin) Emit(event string, data map[string]any, session ...string) {
	m := map[string]any{"event": event, "data": orEmpty(data)}
	if len(session) > 0 {
		m["session"] = session[0]
	}
	p.Send("emit", m)
}

// Notify adds an inbox item (capability `notify`; dropped otherwise). level "" = "info"; nil open is omitted.
func (p *Plugin) Notify(title, body, level string, open map[string]any) {
	m := map[string]any{"title": title, "body": body, "level": cmp.Or(level, "info")}
	if open != nil {
		m["open"] = open
	}
	p.Send("notify", m)
}

// Invalidate makes clients re-run the placement's OpenUI Queries, keeping $state.
func (p *Plugin) Invalidate(placement ...string) {
	p.Send("invalidate", p.at(strings.Join(placement, ""), map[string]any{}))
}

// Log writes an info line, with optional fields, to the plugin log. Use it instead of fmt.Println.
func (p *Plugin) Log(message string, fields ...map[string]any) { p.LogAt("info", message, fields...) }

// LogAt logs at a level: debug|info|warn|error.
func (p *Plugin) LogAt(level, message string, fields ...map[string]any) {
	m := map[string]any{"level": level, "message": message}
	if len(fields) > 0 && len(fields[0]) > 0 {
		m["fields"] = fields[0]
	}
	p.Send("log", m)
}

// ---- plugin -> host requests ----

// Request is a raw request; it blocks until the response. The error is a *RpcError (-32006 once
// stdin is closed) or an encoding error.
func (p *Plugin) Request(method string, params any) (any, error) {
	p.mu.Lock()
	p.nextID++
	id, ch, eof, responder := p.nextID, make(chan reply, 1), p.eof, p.responder
	if !eof && responder == nil {
		p.pending[fmt.Sprint(id)] = ch
	}
	p.mu.Unlock()
	if eof {
		return nil, &RpcError{Code: -32006, Message: "host closed stdin"}
	}
	params = orEmpty(params)
	if err := p.write(map[string]any{"jsonrpc": "2.0", "id": id, "method": method, "params": params}); err != nil {
		return nil, err
	}
	if responder != nil {
		return responder(method, params)
	}
	r := <-ch
	return r.result, r.err
}

// Call is a host-brokered tool call. Returns the ToolOutput {content, is_error, structured?} as-is.
func (p *Plugin) Call(tool string, args map[string]any) (any, error) {
	return p.Request("call", map[string]any{"tool": tool, "args": orEmpty(args)})
}

// Input sends text into a session as the user (capability `input`); returns {session}. "" = unset.
func (p *Plugin) Input(text, session, delivery string) (any, error) {
	m := map[string]any{"text": text, "session": nil}
	if session != "" {
		m["session"] = session
	}
	if delivery != "" {
		m["delivery"] = delivery
	}
	return p.Request("input", m)
}

// Catalog lists what the host offers; topic "" = everything.
func (p *Plugin) Catalog(topic string) (any, error) {
	if topic == "" {
		return p.Request("catalog", nil)
	}
	return p.Request("catalog", map[string]any{"topic": topic})
}

// Ping returns {now_ms}.
func (p *Plugin) Ping() (map[string]any, error) {
	r, err := p.Request("ping", nil)
	m, _ := r.(map[string]any)
	return m, err
}

// ---- I/O ----

func encode(v any) ([]byte, error) {
	var b bytes.Buffer
	enc := json.NewEncoder(&b)
	enc.SetEscapeHTML(false) // never escape <>& (OpenUI source); non-ASCII is written as-is
	err := enc.Encode(v)
	return bytes.TrimSuffix(b.Bytes(), []byte("\n")), err
}

// parse decodes one JSON object, keeping numbers as json.Number so ints round-trip (n stays 1, not 1.0).
func parse(line []byte) map[string]any {
	dec := json.NewDecoder(bytes.NewReader(line))
	dec.UseNumber()
	var v any
	if dec.Decode(&v) != nil || len(bytes.TrimSpace(line[dec.InputOffset():])) > 0 {
		return nil
	}
	m, _ := v.(map[string]any)
	return m
}

// write sends one message as one line under the output lock. The error is an encoding error.
func (p *Plugin) write(msg map[string]any) error {
	b, err := encode(msg)
	if err != nil {
		return err
	}
	if p.sink != nil {
		p.sink(parse(b))
		return nil
	}
	p.outMu.Lock()
	defer p.outMu.Unlock()
	os.Stdout.Write(append(b, '\n'))
	return nil
}

func (p *Plugin) reply(id, result any, e *RpcError) {
	m := map[string]any{"jsonrpc": "2.0", "id": id, "result": result}
	if e != nil {
		m = map[string]any{"jsonrpc": "2.0", "id": id, "error": e}
	}
	if err := p.write(m); err != nil {
		p.reply(id, nil, &RpcError{Code: -32603, Message: "reply not JSON-encodable: " + err.Error()})
	}
}

// ---- dispatch ----

func (p *Plugin) onHello(hp map[string]any) {
	if v, ok := toInt(hp["protocol"]); !ok || v != PROTOCOL {
		p.LogAt("error", fmt.Sprintf("unsupported protocol %v; this SDK speaks %d", hp["protocol"], PROTOCOL))
		os.Exit(1)
	}
	p.Hello, p.Grants, p.Clients = hp, obj(hp["grants"]), obj(hp["clients"])
	p.DataDir, _ = hp["data_dir"].(string)
	if os.Getenv("THEUTH_PLUGIN_DATA") == "" && p.DataDir != "" {
		p.State.mu.Lock()
		p.State.dir, p.State.data = p.DataDir, nil
		p.State.mu.Unlock()
	}
	p.safe("on_start", func() { p.each(p.start) })
}

func (p *Plugin) each(fns []func()) {
	for _, fn := range fns {
		fn()
	}
}

// safe runs hooks or a notification handler; a panic is logged.
func (p *Plugin) safe(what string, fn func()) {
	if _, err := invoke(func() any { fn(); return nil }); err != nil {
		p.LogAt("error", what+" failed: "+err.Error())
	}
}

// invoke runs a handler: a returned error or a panic becomes err.
func invoke(fn func() any) (r any, err error) {
	defer func() {
		if x := recover(); x != nil {
			if err, _ = x.(error); err == nil {
				err = fmt.Errorf("%v", x)
			}
		}
	}()
	r = fn()
	if e, ok := r.(*RpcError); ok && e == nil {
		return nil, nil
	}
	if e, ok := r.(error); ok {
		return nil, e
	}
	return r, nil
}

// dispatch handles one host message on the handler goroutine. Returns false on shutdown.
func (p *Plugin) dispatch(msg map[string]any) bool {
	method, _ := msg["method"].(string)
	pr := obj(msg["params"])
	if id, ok := msg["id"]; ok {
		res, e := p.handle(method, pr, msg)
		p.reply(id, res, e)
		return true
	}
	if method == "shutdown" {
		return false
	}
	p.safe(method, func() { p.notify(method, pr, msg) })
	return true
}

// notify runs the handler or hooks for a host notification; unhandled ones are ignored.
func (p *Plugin) notify(method string, pr, msg map[string]any) {
	hooks := map[string][]func(map[string]any){"granted": p.granted, "lagged": p.lagged,
		"notifications/cancelled": p.cancel}[method]
	switch method {
	case "event":
		v, _ := toInt(pr["version"])
		at, _ := toInt(pr["at_ms"])
		data, _ := pr["data"].(map[string]any)
		ev := Event{Topic: str(pr["topic"]), Type: str(pr["type"]), Session: str(pr["session"]),
			Version: v, AtMs: at, Data: data, Raw: msg}
		for _, k := range []string{ev.Topic + "#" + ev.Type, ev.Topic, ev.Type, "*"} {
			if fn, ok := p.events.fns[k]; ok {
				fn(ev)
				return
			}
		}
	case "granted":
		p.Grants = obj(pr["grants"])
		pr = p.Grants
	case "lagged":
		if len(hooks) == 0 {
			p.LogAt("warn", fmt.Sprintf("lagged: %v events dropped", pr["dropped"]), pr)
		}
	case "notifications/cancelled":
	default:
		if fn, ok := p.notifies.fns[method]; ok {
			fn(pr)
		}
	}
	for _, fn := range hooks {
		fn(pr)
	}
}

func (p *Plugin) handle(method string, pr, msg map[string]any) (any, *RpcError) {
	session, ok := pr["session"]
	if !ok {
		session = obj(obj(pr["_meta"])["theuth"])["session"]
	}
	req := Request{Placement: str(pr["placement"]), Args: pr["args"], Session: str(session), Client: str(pr["client"]),
		Form: pr["form"], Activation: str(pr["activation"]), ID: msg["id"], Raw: msg}
	kinds := map[string]*registry[func(Request) any]{"action": &p.actions, "query": &p.queries,
		"command": &p.commands, "tools/call": &p.tools}
	if reg := kinds[method]; reg != nil {
		req.Name = str(pr["name"])
		if method == "action" || method == "query" {
			req.Name = str(pr[method])
		}
		if method == "tools/call" {
			req.Args = obj(pr["arguments"])
		}
		fn, ok := reg.fns[req.Name]
		if !ok {
			return nil, &RpcError{Code: -32601, Message: method + " not handled: " + req.Name}
		}
		r, err := invoke(func() any { return fn(req) })
		s, isStr := r.(string)
		switch {
		case err != nil:
			return p.fail(method+" "+req.Name, err, method == "tools/call")
		case method == "tools/call":
			return toolResult(r), nil
		case r == nil && method == "action":
			return map[string]any{"ok": true}, nil
		case r == nil && method == "command":
			return map[string]any{}, nil
		case isStr && method == "command":
			return map[string]any{"text": s}, nil
		}
		return r, nil
	}
	fn, ok := p.requests.fns[method]
	if !ok && method == "render" && len(p.render)+len(p.start) > 0 {
		fn, ok = func(map[string]any) any {
			for _, h := range p.render {
				h(req)
			}
			if len(p.render) == 0 {
				p.each(p.start)
			}
			return map[string]any{}
		}, true
	}
	if !ok {
		return nil, &RpcError{Code: -32601, Message: "method not found: " + method}
	}
	r, err := invoke(func() any { return fn(pr) })
	if err != nil {
		return p.fail(method, err, false)
	}
	return r, nil
}

func (p *Plugin) fail(what string, err error, tool bool) (any, *RpcError) {
	var re *RpcError
	if errors.As(err, &re) {
		return nil, re
	}
	p.LogAt("error", what+" failed: "+err.Error())
	if tool {
		return map[string]any{"content": []any{map[string]any{"type": "text", "text": err.Error()}}, "isError": true}, nil
	}
	return nil, &RpcError{Code: -32603, Message: err.Error()}
}

func toolResult(r any) any {
	text := func(s string) []any { return []any{map[string]any{"type": "text", "text": s}} }
	if s, ok := r.(string); ok {
		return map[string]any{"content": text(s), "isError": false}
	}
	b, err := encode(r)
	if err != nil {
		return map[string]any{"content": text(err.Error()), "isError": true}
	}
	out := map[string]any{"content": text(string(b)), "isError": false}
	if m := parse(b); m != nil {
		if _, c := m["content"]; c || m["isError"] != nil {
			return json.RawMessage(b) // already a CallToolResult
		}
		out["structuredContent"] = json.RawMessage(b)
	}
	return out
}

func (p *Plugin) manifest() string {
	var b strings.Builder
	b.WriteString("# registered in code (plugin.yaml is the contract)\n")
	for _, row := range []struct {
		key   string
		names []string
	}{{"tools", p.tools.names}, {"actions", p.actions.names}, {"queries", p.queries.names},
		{"commands", p.commands.names}, {"on", p.events.names}} {
		q := make([]string, len(row.names))
		for i, n := range row.names {
			q[i] = jsStr(n)
		}
		fmt.Fprintf(&b, "%s: [%s]\n", row.key, strings.Join(q, ", "))
	}
	return b.String()
}

// ---- run loop ----

// Run blocks until `shutdown` or stdin EOF, runs OnShutdown hooks and exits 0. With --theuth-manifest
// it prints the registered names and exits 0 without reading stdin.
func (p *Plugin) Run() {
	for _, a := range os.Args[1:] {
		if a == "--theuth-manifest" {
			os.Stdout.WriteString(p.manifest())
			os.Exit(0)
		}
	}
	inbox := make(chan map[string]any, 1)
	go p.read(inbox)
	started := false
	for msg := range inbox {
		if !started {
			if msg["method"] == "hello" {
				p.onHello(obj(msg["params"]))
				started = true
			} else if id, ok := msg["id"]; ok {
				p.reply(id, nil, &RpcError{Code: -32601, Message: "hello first"})
			}
			continue
		}
		if !p.dispatch(msg) {
			break
		}
	}
	p.safe("on_shutdown", func() { p.each(p.shutdown) })
	os.Exit(0)
}

// read is the reader goroutine: it answers pending requests at once and queues everything else for
// the handler goroutine through an unbounded buffer, so it never waits on a handler.
func (p *Plugin) read(inbox chan<- map[string]any) {
	lines := make(chan map[string]any)
	go func() { // unbounded queue between the reader and the handler
		var q []map[string]any
		for in := lines; in != nil || len(q) > 0; {
			var out chan<- map[string]any
			var next map[string]any
			if len(q) > 0 {
				out, next = inbox, q[0]
			}
			select {
			case m, ok := <-in:
				if !ok {
					in = nil
				} else {
					q = append(q, m)
				}
			case out <- next:
				q = q[1:]
			}
		}
		close(inbox)
	}()
	br := bufio.NewReader(os.Stdin)
	for {
		line, err := br.ReadBytes('\n')
		if msg := parse(bytes.TrimRight(line, "\r\n")); msg != nil {
			if _, isReq := msg["method"]; !isReq && msg["id"] != nil {
				p.mu.Lock()
				if ch := p.pending[fmt.Sprint(msg["id"])]; ch != nil {
					ch <- reply{msg["result"], toRpcError(msg)}
					delete(p.pending, fmt.Sprint(msg["id"]))
				}
				p.mu.Unlock()
				continue
			}
			if msg["method"] == "notifications/cancelled" {
				p.markCancelled(msg)
			}
			lines <- msg
		}
		if err != nil {
			break
		}
	}
	p.mu.Lock()
	p.eof = true
	for k, ch := range p.pending {
		ch <- reply{err: &RpcError{Code: -32006, Message: "host closed stdin"}}
		delete(p.pending, k)
	}
	p.mu.Unlock()
	close(lines)
}

func (p *Plugin) markCancelled(msg map[string]any) {
	p.mu.Lock()
	p.cancelled[fmt.Sprint(obj(msg["params"])["requestId"])] = true
	p.mu.Unlock()
}

// ---- testing ----

// Drive runs host messages (map[string]any or JSON strings) through the dispatcher on this goroutine,
// without stdin, and returns what the plugin sent. responder answers the plugin's requests (return a
// *RpcError to fail); nil answers -32601.
func (p *Plugin) Drive(lines []any, responder func(method string, params any) (any, error)) []map[string]any {
	var out []map[string]any
	if responder == nil {
		responder = func(m string, _ any) (any, error) {
			return nil, &RpcError{Code: -32601, Message: "drive(): no responder for " + m}
		}
	}
	p.sink, p.responder = func(m map[string]any) { out = append(out, m) }, responder
	defer func() { p.sink, p.responder = nil, nil }()
	for _, line := range lines {
		msg, _ := line.(map[string]any)
		if s, ok := line.(string); ok {
			msg = parse([]byte(s))
		}
		switch {
		case msg == nil:
		case msg["method"] == "hello":
			p.onHello(obj(msg["params"]))
		case msg["method"] == "notifications/cancelled":
			p.markCancelled(msg)
			p.dispatch(msg)
		case !p.dispatch(msg):
			p.safe("on_shutdown", func() { p.each(p.shutdown) })
			return out
		}
	}
	return out
}

// ---- state ----

// State is a JSON object persisted to <dir>/state.json (dir: $THEUTH_PLUGIN_DATA, else hello's data_dir,
// else ./.plugin-data). Loaded lazily; Set/Add write it atomically (state.tmp + rename). An unreadable
// file starts empty with a warn log.
type State struct {
	owner *Plugin
	mu    sync.Mutex
	data  map[string]any
	dir   string
}

func (s *State) load() map[string]any {
	if s.data == nil {
		b, err := os.ReadFile(s.path())
		var v any
		if err == nil {
			dec := json.NewDecoder(bytes.NewReader(b))
			dec.UseNumber()
			err = dec.Decode(&v)
		}
		if s.data = obj(v); err != nil && !errors.Is(err, fs.ErrNotExist) {
			s.owner.LogAt("warn", "state.json unreadable, starting empty: "+err.Error())
		}
	}
	return s.data
}

func (s *State) path() string {
	return filepath.Join(cmp.Or(s.dir, os.Getenv("THEUTH_PLUGIN_DATA"), "./.plugin-data"), "state.json")
}

// Get returns the value at key, or def when missing.
func (s *State) Get(key string, def any) any {
	s.mu.Lock()
	defer s.mu.Unlock()
	if v, ok := s.load()[key]; ok {
		return v
	}
	return def
}

// Int returns the integer at key; 0 when missing or not an integer.
func (s *State) Int(key string) int {
	v, _ := toInt(s.Get(key, 0))
	return int(v)
}

// Set stores value at key and saves.
func (s *State) Set(key string, value any) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.load()[key] = value
	return s.save()
}

// Add adds n to the integer at key, saves (a failure is logged) and returns the new value.
func (s *State) Add(key string, n int) int {
	s.mu.Lock()
	defer s.mu.Unlock()
	v, _ := toInt(s.load()[key])
	s.data[key] = int(v) + n
	if err := s.save(); err != nil {
		s.owner.LogAt("error", "state.json not saved: "+err.Error())
	}
	return int(v) + n
}

// Save writes state.json (mkdir -p, state.tmp, rename).
func (s *State) Save() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.save()
}

func (s *State) save() error {
	path := s.path()
	b, err := encode(s.load())
	if err == nil {
		err = os.MkdirAll(filepath.Dir(path), 0o755)
	}
	if err == nil {
		err = os.WriteFile(filepath.Join(filepath.Dir(path), "state.tmp"), b, 0o644)
	}
	if err == nil {
		err = os.Rename(filepath.Join(filepath.Dir(path), "state.tmp"), path)
	}
	return err
}

// ---- helpers ----

func obj(v any) map[string]any {
	if m, ok := v.(map[string]any); ok && m != nil {
		return m
	}
	return map[string]any{}
}

func orEmpty(v any) any {
	if m, ok := v.(map[string]any); v == nil || ok && m == nil {
		return map[string]any{}
	}
	return v
}

func str(v any) string {
	if s, ok := v.(string); ok || v == nil {
		return s
	}
	return fmt.Sprint(v)
}

func toInt(v any) (int64, bool) {
	if n, ok := v.(json.Number); ok {
		i, err := n.Int64()
		return i, err == nil
	}
	if r := reflect.ValueOf(v); r.CanInt() {
		return r.Int(), true
	}
	return 0, false
}

// toRpcError is the response's error object as an error, or nil when it has none.
func toRpcError(msg map[string]any) error {
	e, ok := msg["error"]
	if !ok {
		return nil
	}
	m := obj(e)
	code, ok := toInt(m["code"])
	if !ok {
		code = -32603
	}
	return &RpcError{Code: int(code), Message: str(m["message"]), Data: m["data"]}
}

// jsEsc is JSON string escaping without ASCII or HTML escaping (Python json.dumps, ensure_ascii=False).
var jsEsc = func() *strings.Replacer {
	pairs := []string{`"`, `\"`, `\`, `\\`, "\n", `\n`, "\r", `\r`, "\t", `\t`, "\b", `\b`, "\f", `\f`}
	for c := rune(0); c < 0x20; c++ {
		pairs = append(pairs, string(c), fmt.Sprintf(`\u%04x`, c))
	}
	return strings.NewReplacer(pairs...)
}()

func jsStr(s string) string { return `"` + jsEsc.Replace(s) + `"` }
