-- Runs on the tab's machine via `nvim --clean -l`. Only the editor daemon
-- loads the user's configuration; helper and UI clients stay clean.
local M = {}
local uv = vim.uv or vim.loop

-- Names the program only: arguments and environments may carry private values.
function M.failure(program, result)
  local how = result.code == 124 and "timed out (exit code 124)" or ("failed (exit code " .. tostring(result.code) .. ")")
  local detail = vim.trim(result.stderr or "")
  if detail == "" then detail = vim.trim(result.stdout or "") end
  return program .. " " .. how .. (detail ~= "" and (": " .. detail) or "")
end

local function run(argv, options)
  local r = vim.system(argv, vim.tbl_extend("force", { text = true, timeout = 10000 }, options or {})):wait()
  if r.code ~= 0 then error(M.failure(argv[1], r), 0) end
  return r.stdout or ""
end

local function core(method, params)
  local input = vim.json.encode({ jsonrpc = "2.0", id = 1, method = "hello", params = vim.empty_dict() }) .. "\n"
    .. vim.json.encode({ jsonrpc = "2.0", id = 2, method = method, params = params }) .. "\n"
  local text = run({ "hc", "connect" }, { stdin = input })
  for line in text:gmatch("[^\n]+") do
    local reply = vim.json.decode(line)
    if reply.id == 2 then
      if reply.error then error(tostring(reply.error.message or ("herdr-core refused " .. method)), 0) end
      return reply.result
    end
  end
  error("herdr-core gave no reply to " .. method)
end

-- An unavailable or unfamiliar document is not proof that a tab was closed.
function M.find_tab(doc, space, tab)
  if type(doc) ~= "table" or type(doc.version) ~= "number" or doc.version < 1 then return nil end
  local value = doc.value
  if type(value) ~= "table" or type(value.spaces) ~= "table" then return nil end
  for _, s in ipairs(value.spaces) do
    if type(s) ~= "table" or type(s.id) ~= "string" or type(s.tabs) ~= "table" then return nil end
    for _, t in ipairs(s.tabs) do
      if type(t) ~= "table" or type(t.id) ~= "string" then return nil end
    end
  end
  for _, s in ipairs(value.spaces) do
    for _, t in ipairs(s.tabs) do
      if s.id == space and t.id == tab then return t end
    end
  end
  return false
end

local function read(path)
  local f = io.open(path, "rb")
  if not f then return nil end
  local data = f:read("*a"); f:close()
  return data
end

local function write(path, value)
  local f = assert(io.open(path .. ".tmp", "wb"))
  f:write(vim.json.encode(value)); f:close()
  assert(uv.fs_rename(path .. ".tmp", path))
end

local function paths(ctx)
  assert(uv.os_uname().sysname ~= "Windows_NT", "Remote Neovim currently requires a POSIX host")
  local root = uv.os_tmpdir() .. "/herdr-nvim-remote-" .. uv.getuid()
  local stat = uv.fs_lstat(root)
  if not stat then uv.fs_mkdir(root, 448); stat = uv.fs_lstat(root) end
  assert(stat and stat.type == "directory" and stat.uid == uv.getuid(), "Unsafe remote runtime directory")
  assert(uv.fs_chmod(root, 448)) -- 0700
  return root .. "/" .. ctx.key
end

local function alive(pid)
  return pid and uv.kill(pid, 0) ~= nil
end

local function metadata(base)
  local text = read(base .. ".json")
  return text and vim.json.decode(text) or nil
end

local function evaluate(ctx, lua)
  local expr = "luaeval(" .. vim.json.encode(lua) .. ")"
  return vim.json.decode(vim.trim(run({ ctx.nvim, "--clean", "--headless", "--server", paths(ctx) .. ".sock", "--remote-expr", expr })))
end

function M.dead_owner(meta, is_alive)
  return type(meta) == "table" and type(meta.pid) == "number" and meta.pid > 0 and not is_alive(meta.pid)
end

M.START_GRACE = 30

-- A live daemon without a socket is starting only for a while; past that it
-- lost its socket and only `stop --force` can end it.
function M.unsocketed(meta, is_alive, now)
  if type(meta) ~= "table" or type(meta.pid) ~= "number" or meta.pid <= 0 or not is_alive(meta.pid) then
    return { running = false, starting = false }
  end
  if type(meta.started) == "number" and now - meta.started < M.START_GRACE then
    return { running = false, starting = true }
  end
  return { running = false, starting = false, orphan = true, pid = meta.pid }
end

-- The live daemon terminals in a tab's terminal list.
function M.live_daemons(list)
  local out = {}
  for _, t in ipairs(list or {}) do
    if t.state == "live" and type(t.labels) == "table" and t.labels["herdr-nvim.remote-role"] == "daemon" then
      out[#out+1] = t.address
    end
  end
  return out
end

-- A snapshot with the tab's live daemon terminals. One without a socket or
-- metadata is starting only while another action holds the lock (`ensure`
-- holds it until the socket appears); otherwise it is an orphan too.
function M.classify(state, daemons, locked)
  if state.running or state.starting or state.orphan or #daemons == 0 then return state end
  if locked then return { running = false, starting = true } end
  return { running = false, starting = false, orphan = true }
end

M.ORPHAN = "The remote Neovim lost its socket, so its unsaved buffers cannot be checked; "
  .. "run herdr-nvim app-remote-stop --force to end it (unsaved edits survive only in its swap files)"

-- What `stop` does: refuse, quit (through the socket), kill (an orphan, only
-- with force), forget (a dead owner's files) or none.
function M.stop_plan(state, daemons, force)
  if state.running then
    if #state.modified > 0 then return { action = "refuse", reason = "Save the remote Neovim's unsaved buffers before stopping it" } end
    return { action = "quit" }
  end
  if state.starting then return { action = "refuse", reason = "The remote Neovim is still starting; retry when it is ready" } end
  if state.orphan then
    if not force then return { action = "refuse", reason = M.ORPHAN } end
    return { action = "kill", terminals = daemons, pid = state.pid }
  end
  if state.stale then return { action = "forget" } end
  return { action = "none" }
end

-- Whether `ps` shows the daemon's Neovim (`daemon_argv`): a recorded PID may
-- since belong to another process.
function M.owns(args, socket)
  return type(args) == "string" and args:find("--listen " .. socket, 1, true) ~= nil
end

local function snapshot(ctx)
  local base = paths(ctx)
  if not uv.fs_stat(base .. ".sock") then return M.unsocketed(metadata(base), alive, os.time()) end
  local ok, result = pcall(evaluate, ctx, [[(function()
    local modified = {}
    for _, b in ipairs(vim.fn.getbufinfo({bufmodified=1})) do modified[#modified+1] = b.name end
    return vim.json.encode({file=vim.api.nvim_buf_get_name(0),cursor=vim.api.nvim_win_get_cursor(0),cwd=vim.fn.getcwd(),modified=modified})
  end)()]])
  if not ok then
    local meta = metadata(base)
    if not M.dead_owner(meta, alive) then error("The remote Neovim is not answering; no replacement was started: " .. tostring(result)) end
    return { running = false, stale = true }
  end
  result.running = true; result.socket = base .. ".sock"; result.nvim = vim.fn.exepath(ctx.nvim)
  return result
end

local function terminal_ids(node, out)
  if type(node) ~= "table" then return end
  if node.content and node.content.terminal then out[#out+1] = node.content.terminal end
  for _, child in ipairs(node.children or {}) do terminal_ids(child, out) end
end

local function lookup(ctx)
  local doc = core("doc.get", { name = "herdr-app" })
  local tab = M.find_tab(doc, ctx.space, ctx.tab)
  assert(tab, "The remote tab is closed or its document is unavailable")
  return doc, tab
end

local function prepare(ctx, tab)
  local cwd = ctx.cwd
  if not cwd then
    local ids = {}; terminal_ids(tab.root, ids)
    local terminal = ctx.terminal or ids[1]
    if terminal then
      local facts = core("terminal.get", { terminal = terminal })
      cwd = facts.foreground and facts.foreground.cwd or facts.cwd
    end
    cwd = cwd or tab.folder or uv.os_homedir()
  end
  if cwd == "" then cwd = uv.os_homedir() end
  local real = uv.fs_realpath(cwd)
  assert(real and uv.fs_stat(real).type == "directory", "No remote folder: " .. cwd)
  ctx.cwd = real
  local ok, top = pcall(run, { "git", "-C", real, "rev-parse", "--show-toplevel" })
  ctx.root = ok and vim.trim(top) or real
  return ctx
end

local function terminals(ctx)
  return core("terminal.list", { labels = { ["herdr-nvim.remote"] = ctx.key } }).terminals
end

M.LOCK_GRACE = 5
-- serve ends Neovim with SIGTERM, then SIGKILL 2 s later, then waits 2 s more.
M.STOP_WAIT = 6000

-- The lock is created before its PID is written, so a lock without one is
-- held only until a kill between the two could have left it behind.
function M.stale_lock(text, age, is_alive)
  local pid = tonumber(text)
  if pid and pid > 0 and pid % 1 == 0 then return not is_alive(pid) end
  return age > M.LOCK_GRACE
end

local function held(lock)
  local stat = uv.fs_stat(lock)
  if not stat then return false end
  return not M.stale_lock(read(lock), os.time() - stat.mtime.sec, alive)
end

local function ensure(ctx, script)
  local base = paths(ctx)
  local lock = base .. ".lock"
  local fd = uv.fs_open(lock, "wx", 384)
  if not fd then
    if held(lock) then error("Another remote Neovim action is running for this tab") end
    uv.fs_unlink(lock); fd = assert(uv.fs_open(lock, "wx", 384))
  end
  uv.fs_write(fd, tostring(uv.os_getpid())); uv.fs_close(fd)
  local ok, result = xpcall(function()
    local state = snapshot(ctx)
    if state.running then return state end
    -- This action holds the lock, so no other one is starting a daemon.
    state = M.classify(state, M.live_daemons(terminals(ctx)), false)
    assert(not state.starting, "The existing remote Neovim is still starting; no replacement was started")
    assert(not state.orphan, M.ORPHAN .. ", then open it again")
    if state.stale then uv.fs_unlink(base .. ".sock"); uv.fs_unlink(base .. ".json") end
    core("terminal.run", {
      argv = { ctx.nvim, "--clean", "-l", script, "serve", vim.json.encode(ctx) },
      env = ctx.env, cwd = ctx.root, name = "hnvim-" .. ctx.key, no_parent = true, rm = true,
      labels = { ["herdr-nvim.remote"] = ctx.key, ["herdr-nvim.remote-role"] = "daemon" },
    })
    assert(vim.wait(8000, function() return uv.fs_stat(base .. ".sock") ~= nil end, 100),
      "Remote Neovim did not start; inspect " .. base .. ".log")
    return snapshot(ctx)
  end, debug.traceback)
  uv.fs_unlink(lock)
  if not ok then error(result) end
  return result
end

function M.daemon_argv(ctx)
  return { ctx.nvim, "--headless", "--listen", paths(ctx) .. ".sock" }
end

-- One watchdog poll. Only a readable document without the tab closes it; a
-- problem is logged when it starts or changes, not on every poll.
function M.watch(last, ok, tab)
  local problem
  if not ok then
    problem = "cannot read the layout document: " .. tostring(tab)
  elseif tab == nil then
    problem = "the layout document is unavailable or unfamiliar"
  end
  local line
  if problem and problem ~= last then
    line = "herdr-nvim watchdog: " .. problem .. "\n"
  elseif not problem and last then
    line = "herdr-nvim watchdog: the layout document is readable again\n"
  end
  return ok and tab == false, line, problem
end

local function serve(ctx)
  local base = paths(ctx)
  local log = assert(io.open(base .. ".log", "w"))
  local done, stopping = false, false
  local env = vim.fn.environ(); env.HC_SCOPE = nil; env.HC_TERMINAL_ID = nil
  local job = vim.system(M.daemon_argv(ctx), {
    cwd = ctx.root, env = env, clear_env = true,
    stdout = function(_, data) if data then log:write(data); log:flush() end end,
    stderr = function(_, data) if data then log:write(data); log:flush() end end,
  }, function(result) log:write("\nNeovim exit code " .. result.code .. "\n"); done = true end)
  write(base .. ".json", { pid = job.pid, started = os.time() })
  local signals = {}
  for _, name in ipairs({ "sigterm", "sighup" }) do
    local signal = uv.new_signal()
    signal:start(name, function() stopping = true end)
    signals[#signals+1] = signal
  end
  local problem
  while not done and not stopping do
    local ok, tab = pcall(function() return M.find_tab(core("doc.get", { name = "herdr-app" }), ctx.space, ctx.tab) end)
    local closed, line
    closed, line, problem = M.watch(problem, ok, tab)
    if line then log:write(line); log:flush() end
    if closed then break end
    vim.wait(5000, function() return done or stopping end, 100)
  end
  if not done then
    job:kill(15)
    if not vim.wait(2000, function() return done end, 50) then job:kill(9); vim.wait(2000, function() return done end, 50) end
  end
  for _, signal in ipairs(signals) do signal:stop(); signal:close() end
  uv.fs_unlink(base .. ".sock"); uv.fs_unlink(base .. ".json")
  log:close()
end

-- Ends an orphan: its daemon terminals (serve stops Neovim with SIGTERM,
-- which keeps swap files) and its recorded Neovim, when that PID is still it.
-- Done only once neither is alive; their files are then stale.
local function kill(ctx, base, plan)
  for _, address in ipairs(plan.terminals) do core("terminal.kill", { terminal = address }) end
  local pid = plan.pid
  if pid and alive(pid) then
    local ok, args = pcall(run, { "ps", "-o", "args=", "-p", tostring(pid) })
    if not (ok and M.owns(args, base .. ".sock")) then pid = nil end
  else
    pid = nil
  end
  if pid then uv.kill(pid, 15) end
  local function gone()
    return not (pid and alive(pid)) and #M.live_daemons(terminals(ctx)) == 0
  end
  if not vim.wait(M.STOP_WAIT, gone, 250) then
    if pid and alive(pid) then uv.kill(pid, 9) end
    assert(vim.wait(2000, gone, 250), "Remote Neovim did not stop; inspect " .. base .. ".log")
  end
  uv.fs_unlink(base .. ".sock"); uv.fs_unlink(base .. ".json")
end

local function stop(ctx)
  local base = paths(ctx)
  local state = snapshot(ctx)
  local daemons = state.running and {} or M.live_daemons(terminals(ctx))
  local plan = M.stop_plan(M.classify(state, daemons, held(base .. ".lock")), daemons, ctx.force)
  if plan.action == "refuse" then error(plan.reason, 0) end
  if plan.action == "quit" then
    run({ ctx.nvim, "--clean", "--headless", "--server", state.socket, "--remote-send", "<C-\\><C-n><Cmd>qa<CR>" })
    assert(vim.wait(4000, function() return uv.fs_stat(state.socket) == nil end, 100),
      "Remote Neovim did not quit; check for unsaved buffers or a prompt")
  elseif plan.action == "kill" then
    kill(ctx, base, plan)
  elseif plan.action == "forget" then
    uv.fs_unlink(base .. ".sock"); uv.fs_unlink(base .. ".json")
  end
  return { running = false }
end

local function request(op, ctx, script)
  if op == "serve" then serve(ctx); return end
  if op == "stop" then return stop(ctx) end
  local doc, tab = lookup(ctx)
  if op == "info" then return { doc = doc, terminals = terminals(ctx) } end
  if op == "current" then
    local state = snapshot(ctx)
    local stat = state.running and state.file ~= "" and uv.fs_stat(state.file)
    if not state.running then prepare(ctx, tab) end
    return { file = stat and stat.type == "file" and state.file or vim.NIL,
      watch = vim.NIL, tab_root = state.running and state.cwd or ctx.root }
  end
  if op == "open" then
    prepare(ctx, tab)
    local path = ctx.file
    if path then
      if path:sub(1, 2) == "~/" then path = uv.os_homedir() .. path:sub(2) end
      if path:sub(1, 1) ~= "/" then path = ctx.cwd .. "/" .. path end
      local stat = uv.fs_stat(path)
      assert(stat and stat.type == "file", "No remote file: " .. path)
    end
    local state = ensure(ctx, script)
    if path then
      local lua = "(function() vim.cmd.edit(vim.fn.fnameescape(" .. string.format("%q", path) .. ")); "
      if ctx.line then lua = lua .. string.format("vim.api.nvim_win_set_cursor(0,{math.min(%d,vim.api.nvim_buf_line_count(0)),%d}); ", math.max(1, ctx.line), math.max(0, (ctx.col or 1)-1)) end
      evaluate(ctx, lua .. "return vim.json.encode(true) end)()")
    end
    return state
  end
  error("Unknown remote Neovim operation: " .. op)
end

function M.parse_arguments(argv)
  local text = assert(argv[2], "Missing remote context")
  return argv[1], vim.json.decode(text), argv[0]
end

function M.main(argv)
  local op, ctx, script = M.parse_arguments(argv)
  local result = request(op, ctx, script)
  if result then io.stdout:write(vim.json.encode(result) .. "\n") end
end

if ... == nil then
  local ok, err = pcall(M.main, arg)
  if not ok then io.stderr:write(tostring(err) .. "\n"); os.exit(1) end
end
return M
