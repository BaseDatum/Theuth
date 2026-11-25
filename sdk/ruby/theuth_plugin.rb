# theuth plugin SDK for Ruby 2.7+, stdlib only (design 216). Vendored: copy this file beside main.rb.
#
#   require_relative "theuth_plugin"
#   plugin.action("inc") { plugin.state.add("n", 1) }
#   plugin.run
#
# plugin.yaml is the contract: the host learns tools, subscriptions and placements from it, never by
# running this file. `ruby main.rb --theuth-manifest` prints what the code registered, to compare.
#
# Stdout is the protocol. `$stdout` (puts/print/p) is redirected to `log` while `run` is active; stderr is
# the log file. Handlers run one at a time on the main thread; a reader thread reads stdin, so
# `call`/`request` from a handler never deadlocks. Raw hatch: `send`, `request`, `on_request`, `on_notify`.
# Every registration takes a block or a callable; a callable of arity 0 is called without arguments.
require "json"
require "fileutils"
require "set"
require "stringio"

module Theuth
  PROTOCOL = 1

  # A JSON-RPC error. Raise it from a handler to answer with this error; `call`/`request` raise it.
  class RpcError < StandardError
    attr_reader :code, :data, :retry_after_ms

    def initialize(code, message, data = nil)
      super(message)
      @code, @data = code, data
      @retry_after_ms = data.is_a?(Hash) ? data["retry_after_ms"] : nil
    end

    def to_h
      data.nil? ? { "code" => code, "message" => message } : { "code" => code, "message" => message, "data" => data }
    end
  end

  # A host request: action / query / command / tools/call / render / other. `args` and `raw` are Hashes.
  # `form` is nil for OpenUI actions from the desktop: form field values arrive in `args` (through `$state`).
  Request = Struct.new(:placement, :name, :args, :session, :client, :form, :activation, :id, :raw,
                       keyword_init: true)
  # A host event. `data` is the JSON object the host sent (string keys).
  Event = Struct.new(:topic, :version, :type, :session, :at_ms, :data, :raw, keyword_init: true)

  # A Hash persisted to `<data dir>/state.json` (string keys). Every `set`/`add` writes it atomically.
  # Dir: $THEUTH_PLUGIN_DATA, else hello's data_dir, else ./.plugin-data. Loaded on first use.
  class State
    attr_writer :dir # :nodoc: set from hello.data_dir

    def initialize(owner); @owner = owner; end
    def get(key, default = nil); load.fetch(key.to_s, default); end
    # The integer at `key`; 0 when missing or not an integer.
    def int(key); v = get(key, 0); v.is_a?(Integer) ? v : 0; end
    def set(key, value); load[key.to_s] = value; save; end
    # Read-modify-write; saves and returns the new value.
    def add(key, n = 1); v = int(key) + n; set(key, v); v; end
    def all; load; end
    def reset; @data = nil; end # :nodoc: forget the loaded data (data dir changed)
    def path; File.join(@dir || ENV["THEUTH_PLUGIN_DATA"] || "./.plugin-data", "state.json"); end

    # mkdir -p, write and fsync state.tmp, rename to state.json.
    def save
      FileUtils.mkdir_p(File.dirname(path))
      tmp = File.join(File.dirname(path), "state.tmp")
      File.open(tmp, "w") do |f|
        f.write(JSON.generate(load))
        f.flush
        f.fsync
      end
      File.rename(tmp, path)
      nil
    end

    private

    def load
      @data ||= begin
        d = JSON.parse(File.read(path, encoding: "UTF-8"))
        d.is_a?(Hash) ? d : {}
      rescue Errno::ENOENT
        {}
      rescue SystemCallError, JSON::ParserError, EncodingError => e
        @owner.log("state.json unreadable, starting empty: #{e.message}", level: "warn")
        {}
      end
    end
  end

  # The OpenUI source `ui(text:, buttons:)` sends: deterministic, so equal input never remounts.
  def self.openui_card(text, buttons)
    buttons ||= []
    lines = [buttons.empty? ? "root = Card([body])" : "root = Card([body, controls])",
             "body = TextContent(#{JSON.generate(text)})"]
    calls = buttons.each_with_index.map do |(label, action, style), i|
      lines << "a#{i} = Mutation(#{JSON.generate(action)}, {})"
      "Button(#{JSON.generate(label)}, @Run(a#{i}), #{JSON.generate(style || 'secondary')})"
    end
    lines << "controls = Buttons([#{calls.join(', ')}])" unless buttons.empty?
    lines.join("\n")
  end

  # Replaces `$stdout` during run: each printed line becomes a `log` notification.
  class LogWriter < StringIO
    def initialize(owner); super(); @owner, @buf = owner, +""; end

    def write(*parts)
      s = parts.join
      @buf << s
      while (i = @buf.index("\n"))
        @owner.log(@buf.slice!(0, i + 1).chomp, fields: { "stdout" => true })
      end
      s.bytesize
    end
  end

  class Plugin
    # The process-wide plugin; the global `plugin` method returns it.
    def self.instance; @instance ||= new; end

    attr_reader :state, :hello, :grants, :data_dir, :clients

    # Ids of host requests the host cancelled: a frozen snapshot (the reader thread adds to the set).
    def cancelled; @lock.synchronize { @cancelled.dup.freeze }; end

    def initialize
      @state, @hello, @grants, @data_dir, @clients = State.new(self), {}, {}, nil, {}
      @cancelled = Set.new # ids of host requests the host cancelled (notifications/cancelled)
      @handlers = Hash.new { |h, k| h[k] = {} }
      @hooks = Hash.new { |h, k| h[k] = [] }
      @lock, @pending, @next_id, @closed = Mutex.new, {}, 0, false
      @sink = @responder = @reader = nil
      @out = $stdout
    end

    # ---- registration: a block or a callable ----
    # Event handler. `name`: `agent.events#TurnFinished`, `agent.events`, `turns.limit_reached`, or `*`.
    def on(name, fn = nil, &blk); reg(:event, name, fn || blk); end
    # `action` request. Return a JSON value (nil -> {"ok": true}); raise RpcError for an error.
    def action(name, fn = nil, &blk); reg(:action, name, fn || blk); end
    # OpenUI `Query(name, …)`. The return value is the query's value.
    def query(name, fn = nil, &blk); reg(:query, name, fn || blk); end
    # `/plugin:name` command. Return a String (-> {"text": …}), a Hash (sent as-is) or nil (-> {}).
    def command(name, fn = nil, &blk); reg(:command, name, fn || blk); end
    # `tools/call` for a tool declared in plugin.yaml. Return a String, a CallToolResult Hash, or any JSON.
    def tool(name, fn = nil, &blk); reg(:tool, name, fn || blk); end
    # Raw hatch: answer a host request method the helpers do not wrap. Called with params; returns result.
    def on_request(method, fn = nil, &blk); reg(:request, method, fn || blk); end
    # Raw hatch: handle a host notification method. Called with the params.
    def on_notify(method, fn = nil, &blk); reg(:notify, method, fn || blk); end

    # Hooks (several allowed, run in order).
    # After `hello`, before the loop. Paint here. Also re-run on `render` unless on_render is set.
    def on_start(fn = nil, &blk); hook(:start, fn || blk); end
    def on_shutdown(fn = nil, &blk); hook(:shutdown, fn || blk); end
    # Grants changed; `plugin.grants` is already replaced. Called with the new grants.
    def on_granted(fn = nil, &blk); hook(:granted, fn || blk); end
    # Events were dropped (`{dropped, since_ms}`). Default: a warn log.
    def on_lagged(fn = nil, &blk); hook(:lagged, fn || blk); end
    # `notifications/cancelled` (`{requestId, reason}`). The id is already in `plugin.cancelled`.
    def on_cancel(fn = nil, &blk); hook(:cancel, fn || blk); end
    # `render` request (Request with placement, session, client). Send a `ui`; the SDK replies {}.
    def on_render(fn = nil, &blk); hook(:render, fn || blk); end

    # ---- plugin -> host notifications (all return nil, so an action ending in `paint` answers {"ok": true}) ----
    # Raw notification. Shadows Object#send on Plugin; use `__send__` for reflection.
    def send(method, params = nil)
      write("jsonrpc" => "2.0", "method" => method, "params" => params || {})
      nil
    end

    # Dot (ok|busy|warn|error|off|none), badge (Integer, 0 hides; or ≤4 chars), label ≤16, tooltip ≤200.
    def status(dot:, badge: nil, label: nil, tooltip: nil, placement: nil)
      send("status", { "placement" => placement_id(placement), "dot" => dot }
        .merge({ "badge" => badge, "label" => label, "tooltip" => tooltip }.compact))
    end

    # Replace a placement's content. `text` is the fallback for TUI/channels and always sent.
    # `buttons`: [[label, action(, style)]] -> an OpenUI card. `source` is sent unchanged.
    def ui(text:, buttons: nil, source: nil, format: nil, placement: nil)
      if source.nil?
        openui = format.nil? || format == "openui"
        format, source = openui ? ["openui", Theuth.openui_card(text, buttons)] : [format, text]
      end
      send("ui", "placement" => placement_id(placement), "format" => format || "openui",
                 "text" => text, "source" => source)
    end

    # Publish a declared event. An undeclared name is dropped by the host (-32003 in the plugin log);
    # `emit` is a notification, so the SDK cannot see that.
    def emit(event, data, session: nil)
      raise TypeError, "emit data must be a Hash" unless data.is_a?(Hash)
      send("emit", { "event" => event, "data" => data }.merge({ "session" => session }.compact))
    end

    # Inbox item (needs capability `notify`; dropped by the host otherwise).
    def notify(title, body = "", level: "info", open: nil)
      send("notify", { "title" => title, "body" => body, "level" => level }.merge({ "open" => open }.compact))
    end

    # Clients re-run the placement's OpenUI Queries, keeping `$state`.
    def invalidate(placement: nil)
      send("invalidate", "placement" => placement_id(placement))
    end

    def log(message, level: "info", fields: nil)
      p = { "level" => level, "message" => message.to_s }
      p["fields"] = fields if fields && !fields.empty?
      send("log", p)
    end

    # ---- plugin -> host requests (block the handler until the response) ----
    # Raw request. Returns `result` or raises RpcError (-32006 once stdin closed). Not from the reader thread.
    def request(method, params = nil)
      params ||= {}
      return drive_request(method, params) if @responder
      raise "request() from the reader thread would deadlock" if Thread.current == @reader
      q = Queue.new
      rid = @lock.synchronize do
        raise RpcError.new(EOF_ERROR["code"], EOF_ERROR["message"]) if @closed
        @pending[@next_id += 1] = q
        @next_id
      end
      write("jsonrpc" => "2.0", "id" => rid, "method" => method, "params" => params)
      msg = q.pop
      return msg["result"] unless msg.key?("error")
      e = msg["error"] || {}
      raise RpcError.new(e.fetch("code", -32603), e.fetch("message", ""), e["data"])
    end

    # Host-brokered tool call. Returns the ToolOutput `{content, is_error, structured?}` as-is.
    def call(tool, args = nil); request("call", "tool" => tool, "args" => args || {}); end
    # Send text into a session as the user (capability `input`). Returns `{session}`.
    def input(text, session: nil, delivery: nil)
      request("input", { "text" => text, "session" => session }.merge({ "delivery" => delivery }.compact))
    end
    def catalog(topic = nil); request("catalog", topic ? { "topic" => topic } : {}); end
    # Returns `{"now_ms" => …}`.
    def ping; request("ping", {}); end

    # ---- run loop ----
    # `--theuth-manifest`: print what the code registered, exit 0 without reading stdin. Otherwise block
    # until `shutdown` or stdin EOF, then call on_shutdown hooks and exit 0.
    def run
      ($stdout.write(manifest); $stdout.flush; exit 0) if ARGV.include?("--theuth-manifest")
      @out = $stdout
      @out.sync = true
      $stdout = LogWriter.new(self)
      inbox = Queue.new
      @reader = Thread.new { read_loop(inbox) }
      started = false
      while (msg = inbox.pop)
        if started
          break unless dispatch(msg)
        elsif msg["method"] == "hello"
          on_hello(msg["params"] || {})
          started = true
        elsif msg.key?("id")
          reply(msg["id"], error: { "code" => -32601, "message" => "hello first" })
        end
      end
      shutdown_hooks
      $stdout = @out
      @out.flush
      exit!(0) # the reader thread may sit in a blocking read
    end

    # Run host messages (Hashes or JSON strings) through the dispatcher without stdin. Returns what the
    # plugin sent. `responder.call(method, params)` (or the block) answers the plugin's requests.
    def drive(lines, responder = nil, &blk)
      out = []
      @sink = out
      @responder = responder || blk || ->(m, _p) { raise RpcError.new(-32601, "drive(): no responder for #{m}") }
      lines.each do |line|
        msg = JSON.parse(line.is_a?(String) ? line : JSON.generate(line))
        if msg["method"] == "hello"
          on_hello(msg["params"] || {})
        elsif msg["method"] == "notifications/cancelled"
          mark_cancelled(msg)
          dispatch(msg)
        elsif !dispatch(msg)
          shutdown_hooks
          break
        end
      end
      out
    ensure
      @sink = @responder = nil
    end

    # The `--theuth-manifest` text, in registration order.
    def manifest
      rows = { "tools" => :tool, "actions" => :action, "queries" => :query, "commands" => :command, "on" => :event }
      "# registered in code (plugin.yaml is the contract)\n" + rows.map do |k, kind|
        "#{k}: [#{@handlers[kind].keys.map { |n| JSON.generate(n) }.join(', ')}]\n"
      end.join
    end

    private

    def reg(kind, name, fn)
      @handlers[kind][name] = fn || raise(ArgumentError, "#{kind} #{name}: pass a block or a callable")
    end

    def hook(kind, fn)
      (@hooks[kind] << (fn || raise(ArgumentError, "#{kind} hook: pass a block or a callable"))).last
    end

    # Blocks ignore extra arguments; a lambda/Method of arity 0 is called without the request.
    def invoke(fn, *args); fn.respond_to?(:arity) && fn.arity.zero? ? fn.call : fn.call(*args); end

    def placement_id(placement)
      ids = placement ? [placement] : (@grants || {})["placements"] || []
      ids.size == 1 ? ids[0] : raise(ArgumentError, "pass placement: the grant holds #{ids.size} placements #{ids}")
    end

    # JSON.generate leaves non-ASCII and <>& unescaped, as the wire requires.
    def write(msg)
      return @sink << msg if @sink
      line = JSON.generate(msg) + "\n"
      @lock.synchronize { @out.write(line); @out.flush }
    end

    def reply(id, result: nil, error: nil)
      write({ "jsonrpc" => "2.0", "id" => id }.merge(error ? { "error" => error } : { "result" => result }))
    end

    def mark_cancelled(msg); @lock.synchronize { @cancelled << (msg["params"] || {})["requestId"] }; end

    def drive_request(method, params)
      @sink << { "jsonrpc" => "2.0", "id" => @next_id += 1, "method" => method, "params" => params }
      @responder.call(method, params)
    end

    def on_hello(p)
      if p["protocol"] != PROTOCOL
        m = "unsupported protocol #{p['protocol'].inspect}; this SDK speaks #{PROTOCOL}"
        raise m if @sink # drive: fail the test, not the test process
        log(m, level: "error")
        exit!(1)
      end
      @hello, @grants, @data_dir, @clients = p, p["grants"] || {}, p["data_dir"], p["clients"] || {}
      (@state.dir = @data_dir; @state.reset) if (ENV["THEUTH_PLUGIN_DATA"] || "").empty? && @data_dir
      guard("on_start") { @hooks[:start].each { |fn| invoke(fn) } }
    end

    def error_text(e); e.message.empty? ? e.class.name : e.message; end

    # Run a notification handler or hook; an exception is logged at error and the process stays up.
    def guard(what)
      yield
    rescue StandardError => e
      log("#{what} failed: #{e.inspect}", level: "error")
    end

    def shutdown_hooks; guard("on_shutdown") { @hooks[:shutdown].each { |fn| invoke(fn) } }; end

    # Handle one host message on the handler thread. Returns false on shutdown.
    def dispatch(msg)
      method, p = msg["method"], msg["params"] || {}
      if msg.key?("id")
        begin
          reply(msg["id"], result: handle_request(method, p, msg))
        rescue RpcError => e
          reply(msg["id"], error: e.to_h)
        rescue StandardError => e # a handler bug answers -32603, the process stays up
          log("#{method} failed: #{e.inspect}", level: "error")
          reply(msg["id"], error: { "code" => -32603, "message" => error_text(e) })
        end
        return true
      end
      return false if method == "shutdown"
      guard(method) { notification(method, p, msg) }
      true
    end

    def notification(method, p, msg)
      case method
      when "event"
        ev = Event.new(topic: p["topic"], version: p["version"], type: p["type"], session: p["session"],
                       at_ms: p["at_ms"], data: p["data"], raw: msg)
        h = @handlers[:event]
        key = ["#{ev.topic}##{ev.type}", ev.topic, ev.type, "*"].find { |k| h.key?(k) }
        invoke(h[key], ev) if key
      when "granted"
        @grants = p["grants"] || {}
        @hooks[:granted].each { |fn| invoke(fn, @grants) }
      when "lagged"
        log("lagged: #{p['dropped']} events dropped", level: "warn", fields: p) if @hooks[:lagged].empty?
        @hooks[:lagged].each { |fn| invoke(fn, p) }
      when "notifications/cancelled"
        @hooks[:cancel].each { |fn| invoke(fn, p) }
      else @handlers[:notify][method]&.then { |fn| invoke(fn, p) }
      end
    end

    KINDS = { "action" => [:action, "action"], "query" => [:query, "query"],
              "command" => [:command, "name"], "tools/call" => [:tool, "name"] }.freeze
    EOF_ERROR = { "code" => -32006, "message" => "host closed stdin" }.freeze
    private_constant :KINDS, :EOF_ERROR

    def handle_request(method, p, msg)
      meta = (p["_meta"] || {})["theuth"] || {}
      req = Request.new(placement: p["placement"], args: p["args"], client: p["client"], form: p["form"],
                        session: p.key?("session") ? p["session"] : meta["session"],
                        activation: p["activation"], id: msg["id"], raw: msg)
      kind, key = KINDS[method]
      if kind
        req.name = p[key]
        req.args = p["arguments"] || {} if kind == :tool
        fn = @handlers[kind][req.name] or raise RpcError.new(-32601, "#{method} not handled: #{req.name}")
        return tool_call(fn, req) if kind == :tool
        r = invoke(fn, req)
        return r.nil? ? { "ok" => true } : r if kind == :action
        return r.is_a?(String) ? { "text" => r } : (r.nil? ? {} : r) if kind == :command
        return r
      end
      return invoke(@handlers[:request][method], p) if @handlers[:request].key?(method)
      if method == "render" && !(@hooks[:render].empty? && @hooks[:start].empty?)
        @hooks[:render].each { |fn| invoke(fn, req) }
        @hooks[:start].each { |fn| invoke(fn) } if @hooks[:render].empty?
        return {}
      end
      raise RpcError.new(-32601, "method not found: #{method}")
    end

    # A tool's return value -> CallToolResult; an exception -> isError text (the model must see it).
    def tool_call(fn, req)
      r = invoke(fn, req)
      return { "content" => [{ "type" => "text", "text" => r }], "isError" => false } if r.is_a?(String)
      return r if r.is_a?(Hash) && [:content, "content", :isError, "isError"].any? { |k| r.key?(k) }
      out = { "content" => [{ "type" => "text", "text" => JSON.generate(r) }], "isError" => false }
      out["structuredContent"] = r if r.is_a?(Hash)
      out
    rescue RpcError
      raise
    rescue StandardError => e
      log("tool #{req.name} failed: #{e.inspect}", level: "error")
      { "content" => [{ "type" => "text", "text" => error_text(e) }], "isError" => true }
    end

    # Reader thread: responses go to the waiting request; everything else to the handler queue.
    def read_loop(inbox)
      $stdin.each_line do |raw|
        msg = (JSON.parse(raw.force_encoding(Encoding::UTF_8).chomp) rescue nil) # non-JSON lines are ignored
        next unless msg.is_a?(Hash)
        next @lock.synchronize { @pending.delete(msg["id"]) }&.push(msg) if !msg.key?("method") && msg.key?("id")
        mark_cancelled(msg) if msg["method"] == "notifications/cancelled"
        inbox.push(msg)
      end
    ensure # fail waiting requests; the closed flag makes later ones fail at once
      @lock.synchronize { @closed = true; @pending.each_value { |q| q.push("error" => EOF_ERROR) }; @pending.clear }
      inbox.push(nil)
    end
  end
end

# The plugin singleton: `plugin.action("inc") { … }`, `plugin.run`.
def plugin; Theuth::Plugin.instance; end
