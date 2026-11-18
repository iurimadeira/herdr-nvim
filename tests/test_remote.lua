local remote = require("herdr-nvim.remote")

T.test("remote: nvim -l arguments decode without forwarding assert's message", function()
  local op, ctx, script = remote.parse_arguments({ [0] = "/tmp/worker.lua", "info", '{"space":"s1","tab":"t1"}' })
  T.eq(op, "info"); T.eq(ctx, { space = "s1", tab = "t1" }); T.eq(script, "/tmp/worker.lua")
end)

T.test("remote: documents are validated before deciding a tab was closed", function()
  local unknown = {
    {}, { version = 0 }, { version = 1, value = {} },
    { version = "1", value = { spaces = {} } },
    { version = 1, value = { spaces = { { id = "s1" } } } },
    { version = 1, value = { spaces = { { id = "s1", tabs = { {} } } } } },
  }
  T.eq(remote.find_tab(nil, "s1", "t1"), nil)
  for _, doc in ipairs(unknown) do T.eq(remote.find_tab(doc, "s1", "t1"), nil) end
  T.eq(remote.find_tab({ version = 1, value = { spaces = {} } }, "s1", "t1"), false)
end)

T.test("remote: tabs are matched inside their owning space", function()
  local tab = { id = "t1", root = { id = "p1" } }
  local doc = { version = 2, value = { spaces = { { id = "s1", tabs = { tab } } } } }
  T.eq(remote.find_tab(doc, "s1", "t1"), tab)
  T.eq(remote.find_tab(doc, "s2", "t1"), false)
  T.eq(remote.find_tab(doc, "s1", "t2"), false)
end)

T.test("remote: stale sockets are reaped only for a known dead owner", function()
  T.ok(remote.dead_owner({ pid = 123 }, function(pid) T.eq(pid, 123); return false end))
  T.ok(not remote.dead_owner({ pid = 123 }, function() return true end))
  for _, meta in ipairs({ {}, { pid = 0 }, { pid = "123" } }) do
    T.ok(not remote.dead_owner(meta, function() error("invalid metadata must not probe a PID") end))
  end
  T.ok(not remote.dead_owner(nil, function() return false end))
end)

local function run_helper(op, ctx, respond)
  local system, stdout = vim.system, io.stdout
  local calls, output = {}, ""
  vim.system = function(argv, opts)
    if respond then
      local result = respond(argv, opts, calls)
      if result then return { wait = function() return result end } end
    end
    if argv[1] == "git" then
      calls[#calls+1] = "git"
      return { wait = function() return { code = 1, stderr = "not a repository" } end }
    end
    T.eq(argv, { "hc", "connect" }) -- info and invalid opens must not contact Neovim
    local request = vim.json.decode(opts.stdin:match("[^\n]+\n([^\n]+)"))
    calls[#calls+1] = request.method
    local value = request.method == "doc.get"
      and { version = 1, value = { spaces = { { id = "s1", tabs = { { id = "t1" } } } } } }
      or ctx.reply or { terminals = {} }
    return { wait = function() return { code = 0, stdout = vim.json.encode({ id = 2, result = value }) .. "\n" } end }
  end
  io.stdout = { write = function(_, text) output = output .. text end }
  local ok, err = pcall(remote.main, { [0] = "/tmp/worker.lua", op, vim.json.encode(ctx) })
  vim.system, io.stdout = system, stdout
  return ok, err, output, calls
end

T.test("remote: pane inspection reads one document and never probes the editor", function()
  local ok, err, output, calls = run_helper("info", { space = "s1", tab = "t1", key = "test", cwd = "/deleted/folder" })
  T.ok(ok, err)
  T.eq(calls, { "doc.get", "terminal.list" })
  T.eq(vim.json.decode(output).doc.version, 1)
end)

T.test("remote: invalid files fail before starting or probing a daemon", function()
  local ok, err, _, calls = run_helper("open", {
    space = "s1", tab = "t1", key = "test", cwd = vim.uv.os_tmpdir(),
    file = vim.uv.os_tmpdir() .. "/hnvim-missing-" .. vim.uv.hrtime(),
  })
  T.ok(not ok and tostring(err):find("No remote file", 1, true))
  T.eq(calls, { "doc.get", "git" })
end)

T.test("remote: daemon startup preserves the user's configuration", function()
  if vim.fn.has("win32") == 1 then return end
  local argv = remote.daemon_argv({ nvim = "/remote/bin/nvim", key = "test-startup" })
  T.eq(argv[1], "/remote/bin/nvim")
  T.eq(argv[2], "--headless")
  T.eq(argv[3], "--listen")
  T.eq(#argv, 4)
  T.ok(argv[4]:match("/test%-startup%.sock$"))
end)

T.test("remote: a failing command names itself and its exit code, never its arguments", function()
  for code, how in pairs({ [1] = "hc failed (exit code 1)", [124] = "hc timed out (exit code 124)" }) do
    local ok, err = run_helper("info", { space = "s1", tab = "t1", key = "test" }, function()
      return { code = code, stderr = "", stdout = "" }
    end)
    T.ok(not ok)
    T.eq(tostring(err), how)
  end
  T.eq(remote.failure("git", { code = 128, stderr = "\nfatal: not a repository\n" }),
    "git failed (exit code 128): fatal: not a repository")
  T.eq(remote.failure("nvim", { code = 1, stdout = "E5108: boom" }), "nvim failed (exit code 1): E5108: boom")
end)

T.test("remote: a herdr-core error without a message names the method", function()
  local ok, err = run_helper("info", { space = "s1", tab = "t1", key = "test" }, function(argv)
    if argv[1] == "hc" then return { code = 0, stdout = '{"id":2,"error":{"code":-1}}\n' } end
  end)
  T.ok(not ok)
  T.eq(tostring(err), "herdr-core refused doc.get")
end)

T.test("remote: the watchdog logs each new problem once and closes only on a missing tab", function()
  local closed, line, last = remote.watch(nil, false, "hc failed (exit code 1)")
  T.ok(not closed)
  T.eq(line, "herdr-nvim watchdog: cannot read the layout document: hc failed (exit code 1)\n")
  closed, line, last = remote.watch(last, false, "hc failed (exit code 1)")
  T.ok(not closed); T.eq(line, nil)
  closed, line, last = remote.watch(last, true, nil)
  T.ok(not closed, "an unavailable document is not proof of close")
  T.eq(line, "herdr-nvim watchdog: the layout document is unavailable or unfamiliar\n")
  closed, line, last = remote.watch(last, true, { id = "t1" })
  T.ok(not closed); T.eq(line, "herdr-nvim watchdog: the layout document is readable again\n"); T.eq(last, nil)
  closed, line = remote.watch(last, true, { id = "t1" })
  T.ok(not closed); T.eq(line, nil)
  closed, line = remote.watch(nil, true, false)
  T.ok(closed); T.eq(line, nil)
end)

T.test("remote: a daemon without a socket is starting only within the grace period", function()
  local live = function() return true end
  local now = 1000
  T.eq(remote.unsocketed(nil, live, now), { running = false, starting = false })
  T.eq(remote.unsocketed({ pid = 7, started = now - 1 }, function() return false end, now), { running = false, starting = false })
  T.eq(remote.unsocketed({ pid = 7, started = now - 1 }, live, now), { running = false, starting = true })
  T.eq(remote.unsocketed({ pid = 7, started = now - remote.START_GRACE }, live, now),
    { running = false, starting = false, orphan = true })
  T.eq(remote.unsocketed({ pid = 7 }, live, now), { running = false, starting = false, orphan = true })
end)

T.test("remote: a lock without a live owner is stale, an empty one only after a grace period", function()
  local live = function() return true end
  local dead = function() return false end
  T.ok(not remote.stale_lock("42", 999, live))
  T.ok(remote.stale_lock("42", 0, dead))
  for _, text in ipairs({ "", "garbage", "-3", "1.5" }) do
    T.ok(not remote.stale_lock(text, 0, function() error("no PID to probe") end))
    T.ok(remote.stale_lock(text, remote.LOCK_GRACE + 1, function() error("no PID to probe") end))
  end
  T.ok(remote.stale_lock(nil, math.huge, live))
end)

T.test("remote: stop ends a daemon that lost its socket", function()
  if vim.fn.has("win32") == 1 then return end
  local uv = vim.uv or vim.loop
  local key = "test-orphan-" .. uv.os_getpid() .. "-" .. uv.hrtime()
  local base = uv.os_tmpdir() .. "/herdr-nvim-remote-" .. uv.getuid() .. "/" .. key
  vim.fn.mkdir(vim.fs.dirname(base), "p", 448)
  vim.fn.writefile({ vim.json.encode({ pid = uv.os_getpid(), started = 0 }) }, base .. ".json")
  local killed = {}
  local ok, err, output = run_helper("stop", {
    space = "s1", tab = "t1", key = key,
    reply = { terminals = { { address = "t5", state = "live", labels = { ["herdr-nvim.remote-role"] = "daemon" } } } },
  }, function(argv, opts)
    local request = vim.json.decode(opts.stdin:match("[^\n]+\n([^\n]+)"))
    if request.method == "terminal.kill" then
      killed[#killed+1] = request.params.terminal
      os.remove(base .. ".json")
      return { code = 0, stdout = vim.json.encode({ id = 2, result = vim.empty_dict() }) .. "\n" }
    end
  end)
  os.remove(base .. ".json")
  T.ok(ok, err)
  T.eq(killed, { "t5" })
  T.eq(vim.json.decode(output), { running = false })
end)
