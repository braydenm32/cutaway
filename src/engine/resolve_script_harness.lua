-- A stand-in for DaVinci Resolve's scripting API, for Cutaway's tests: runs
-- the "Build project from a take" script against it and prints every call
-- that shapes the project, one per line.
--
--   luajit resolve_script_harness.lua <script> <config dir> <picker folder or -> <inclusive|exclusive|broken>
--
-- <config dir> is where Cutaway's notes are (resolve-build.txt,
-- last-take.txt). The folder picker answers <picker folder> ("-": cancel).
-- endFrame is counted inclusive (as Resolve documents it), exclusive, or
-- AppendToTimeline fails ("broken": the script falls back to the .fcpxml).

local script, config_dir, picker, mode = arg[1], arg[2], arg[3], arg[4] or "inclusive"
local real_print = print
local function out(...)
  local parts = {}
  for i = 1, select("#", ...) do
    parts[i] = tostring((select(i, ...)))
  end
  real_print(table.concat(parts, "\t"))
end

-- bmd.readdir: a real directory listing, with "*" patterns.
local function glob_to_pattern(g)
  return "^" .. g:gsub("[%^%$%(%)%%%.%[%]%+%-%?]", "%%%0"):gsub("%*", ".*") .. "$"
end
bmd = {
  readdir = function(path)
    local dir, pattern = path:match("^(.*)[/\\]([^/\\]*)$")
    local p = io.popen('ls -1 "' .. dir .. '" 2>/dev/null')
    local entries = {}
    for name in p:lines() do
      if name:match(glob_to_pattern(pattern)) then
        table.insert(entries, { Name = name, IsDir = false })
      end
    end
    p:close()
    return entries
  end,
}

local function item(path)
  local it = { path = path }
  function it:GetClipProperty(key)
    if key == "File Path" then
      return self.path
    end
  end
  function it:LinkProxyMedia(p)
    out("PROXY", self.path, p)
    return true
  end
  return it
end

local function timeline_item(info, length)
  local ti = { info = info }
  function ti:GetDuration()
    return length
  end
  function ti:SetClipEnabled(on)
    out("CLIPENABLED", tostring(on))
    return true
  end
  return ti
end

local function timeline(name)
  local tl = { name = name, video = 1, markers = {} }
  function tl:GetStartFrame()
    return 108000
  end
  function tl:GetTrackCount(kind)
    return kind == "video" and self.video or 1
  end
  function tl:AddTrack(kind)
    if kind == "video" then
      self.video = self.video + 1
    end
    return true
  end
  function tl:SetTrackName(kind, i, n)
    out("TRACKNAME", kind, i, n)
    return true
  end
  function tl:SetTrackEnable(kind, i, on)
    out("TRACKENABLE", kind, i, tostring(on))
    return true
  end
  function tl:DeleteClips(list)
    out("DELETECLIPS", #list)
    return true
  end
  function tl:GetMarkers()
    return self.markers
  end
  function tl:AddMarker(frame, color, n, note, dur)
    self.markers[frame] = n
    out("MARKER", frame, n)
    return true
  end
  return tl
end

local function base_name(p)
  return (p:match("[^/\\]+$"))
end

local pool = {}
function pool:GetRootFolder()
  return { name = "Master" }
end
function pool:AddSubFolder(_, n)
  out("BIN", n)
  return { name = n }
end
function pool:SetCurrentFolder(_)
  return true
end
function pool:ImportMedia(files)
  local items = {}
  for _, f in ipairs(files) do
    out("IMPORT", base_name(f))
    table.insert(items, item(f))
  end
  return items
end
function pool:CreateEmptyTimeline(n)
  out("TIMELINE", n)
  return timeline(n)
end
function pool:AppendToTimeline(list)
  if mode == "broken" then
    return nil
  end
  local got = {}
  for _, c in ipairs(list) do
    local length = c.endFrame - c.startFrame + (mode == "inclusive" and 1 or 0)
    out("APPEND", base_name(c.mediaPoolItem.path), "from=" .. c.startFrame, "end=" .. c.endFrame, "record=" .. c.recordFrame, "track=" .. c.trackIndex, "media=" .. tostring(c.mediaType))
    table.insert(got, timeline_item(c, length))
  end
  return got
end
function pool:DeleteTimelines(list)
  out("DELETETIMELINES", #list)
  return true
end
function pool:ImportTimelineFromFile(path, opts)
  out("IMPORTTIMELINE", base_name(path))
  return timeline(opts.timelineName)
end

local project = {}
function project:SetSetting(k, v)
  out("SETTING", k, v)
  return true
end
function project:GetMediaPool()
  return pool
end
function project:SetCurrentTimeline(tl)
  out("CURRENT", tl.name)
  return true
end

local pm = {}
function pm:CreateProject(n)
  out("PROJECT", n)
  return project
end
function pm:SaveProject()
  out("SAVE")
  return true
end

resolve = {}
function resolve:GetProjectManager()
  return pm
end
fu = {
  RequestDir = function(_, start)
    out("PICKER", start)
    if picker == "-" then
      return nil
    end
    return picker
  end,
}
cutaway_test_dirs = { config_dir }

print = function(s)
  real_print("SAY\t" .. tostring(s))
end
dofile(script)
