-- Cutaway - Build project from a take
--
-- In DaVinci Resolve: Workspace > Scripts > Cutaway - Build project from a take.
-- After Open in Resolve in Cutaway it builds that take straight away;
-- otherwise pick a take folder recorded by Cutaway (it starts at the latest take).
-- You get a new project named after the take, at the take's frame rate and size,
-- with bins for the cameras, the program and the audio, proxies linked,
-- and the live edit as a timeline: your cuts on V1, every camera (and the
-- program) full length on its own track above, switched off, all in sync,
-- the sound below, and the markers.
--
-- Installed (and kept up to date) by Cutaway's Open in Resolve, or from
-- Record > Recording > Editing. Works in the free Resolve.

local resolve = resolve or (Resolve and Resolve()) or bmd.scriptapp("Resolve")
if not resolve then
  print("Cutaway: run this from DaVinci Resolve (Workspace > Scripts).")
  return
end
local fusion = fu or resolve:Fusion()
local sep = package.config:sub(1, 1)

local function say(text)
  print("Cutaway: " .. text)
end

-- Where Cutaway keeps its notes (next to its settings).
local function cutaway_dirs()
  local test = rawget(_G, "cutaway_test_dirs")
  if test then
    return test
  end
  local dirs = {}
  local home = os.getenv("HOME")
  local appdata = os.getenv("APPDATA")
  local xdg = os.getenv("XDG_CONFIG_HOME")
  if home then
    table.insert(dirs, home .. "/Library/Application Support/Cutaway")
  end
  if xdg and xdg ~= "" then
    table.insert(dirs, xdg .. "/cutaway")
  end
  if home then
    table.insert(dirs, home .. "/.config/cutaway")
  end
  if appdata then
    table.insert(dirs, appdata .. "\\Cutaway")
  end
  return dirs
end

local function first_lines(path)
  local f = io.open(path, "r")
  if not f then
    return nil
  end
  local line = f:read("*l")
  local second = f:read("*l")
  f:close()
  return line, second
end

-- Open in Resolve leaves a note: build this take now. Fresh (under 15
-- minutes) it's used, without asking; either way it's used up.
local function take_to_build()
  for _, dir in ipairs(cutaway_dirs()) do
    local path = dir .. sep .. "resolve-build.txt"
    local folder, when = first_lines(path)
    if folder then
      os.remove(path)
      local age = os.time() - (tonumber(when) or 0)
      if folder ~= "" and age >= -60 and age < 15 * 60 then
        return folder
      end
    end
  end
  return nil
end

-- Cutaway notes the latest take here.
local function latest_take()
  for _, dir in ipairs(cutaway_dirs()) do
    local folder = first_lines(dir .. sep .. "last-take.txt")
    if folder and folder ~= "" then
      return folder
    end
  end
  return nil
end

local function list(folder, pattern)
  local out = {}
  for _, entry in ipairs(bmd.readdir(folder .. sep .. pattern) or {}) do
    if not entry.IsDir then
      table.insert(out, folder .. sep .. entry.Name)
    end
  end
  table.sort(out)
  return out
end

local function take_file(folder)
  return list(folder, "*(Resolve).lua")[1]
end

local function file_name(path)
  return (tostring(path):match("[^/\\]+$")) or tostring(path)
end

-- A take without Cutaway's take file: its videos, sound and .fcpxml.
local function take_from_folder(folder)
  local t = { name = file_name(folder), folder = folder, cameras = {}, audio = {}, proxies = {}, markers = {} }
  local seen = {}
  for _, pattern in ipairs({ "*.mp4", "*.mov", "*.MP4", "*.MOV" }) do
    for _, p in ipairs(list(folder, pattern)) do
      if not seen[p] then
        seen[p] = true
        table.insert(t.cameras, p)
      end
    end
  end
  if #t.cameras == 0 then
    return nil
  end
  -- The program first.
  table.sort(t.cameras, function(a, b)
    local pa, pb = file_name(a):find("^Program") ~= nil, file_name(b):find("^Program") ~= nil
    if pa ~= pb then
      return pa
    end
    return a < b
  end)
  for _, pattern in ipairs({ "*.wav", "*.WAV" }) do
    for _, p in ipairs(list(folder, pattern)) do
      if not seen[p] then
        seen[p] = true
        table.insert(t.audio, p)
      end
    end
  end
  t.timeline = list(folder, "*.fcpxml")[1]
  return t
end

local folder = take_to_build()
if not folder then
  folder = fusion:RequestDir(latest_take() or "")
end
if not folder or folder == "" then
  say("no folder chosen.")
  return
end
folder = folder:gsub("[/\\]+$", "")
local take
local file = take_file(folder)
if file then
  local ok, t = pcall(dofile, file)
  if not ok or type(t) ~= "table" then
    say("couldn't read " .. file)
    return
  end
  take = t
else
  take = take_from_folder(folder)
  if not take then
    say("that folder isn't a Cutaway take (there are no videos in it). Pick the take's own folder, the one with Program.mp4 in it.")
    return
  end
end

-- The take may have moved (copied to another drive): its files are in the
-- folder picked, wherever they were recorded.
local function here(path)
  if type(path) ~= "string" or type(take.folder) ~= "string" then
    return path
  end
  local was = take.folder
  local head = path:sub(1, #was)
  local same = head == was
  if not same and sep == "\\" then
    same = head:gsub("/", "\\"):lower() == was:gsub("/", "\\"):lower()
  end
  if not same then
    return path
  end
  local rest = path:sub(#was + 1):gsub("[/\\]", sep)
  return folder .. rest
end
local function all_here(paths)
  local out = {}
  for i, p in ipairs(paths or {}) do
    out[i] = here(p)
  end
  return out
end
take.cameras = all_here(take.cameras)
take.audio = all_here(take.audio)
take.timeline = here(take.timeline)
take.sound = here(take.sound)
for _, c in ipairs(take.edit or {}) do
  c.file = here(c.file)
end
for _, a in ipairs(take.angles or {}) do
  a.file = here(a.file)
end
local proxies = {}
for k, v in pairs(take.proxies or {}) do
  proxies[here(k)] = here(v)
end
take.proxies = proxies

local pm = resolve:GetProjectManager()
local name = take.name
local project = pm:CreateProject(name)
if not project then
  name = take.name .. " " .. os.date("%H%M%S")
  project = pm:CreateProject(name)
end
if not project then
  say("Resolve couldn't make a project called " .. name)
  return
end
-- The take's frame rate and picture size, before any media or timeline.
if take.fps then
  local fps = tostring(take.fps)
  project:SetSetting("timelineFrameRate", fps)
  project:SetSetting("timelinePlaybackFrameRate", fps)
end
if take.width and take.height then
  project:SetSetting("timelineResolutionWidth", tostring(take.width))
  project:SetSetting("timelineResolutionHeight", tostring(take.height))
end

local pool = project:GetMediaPool()
local root = pool:GetRootFolder()
local function bin(title, files)
  if #files == 0 then
    return nil, {}
  end
  local b = pool:AddSubFolder(root, title)
  pool:SetCurrentFolder(b)
  local items = pool:ImportMedia(files) or {}
  return b, items
end

local cameras_bin, cameras = bin("Cameras", take.cameras or {})
local _, sounds = bin("Audio", take.audio or {})

-- Proxies (small copies) for smooth editing.
local linked = 0
for _, item in ipairs(cameras) do
  local path = item:GetClipProperty("File Path")
  local proxy = take.proxies and take.proxies[path]
  if proxy and item:LinkProxyMedia(proxy) then
    linked = linked + 1
  end
end

-- The media pool's clips by file (and by name, should Resolve spell the
-- path its own way).
local function norm(p)
  p = tostring(p or ""):gsub("\\", "/")
  if sep == "\\" then
    p = p:lower()
  end
  return p
end
local by_path, by_name = {}, {}
for _, group in ipairs({ cameras, sounds }) do
  for _, item in ipairs(group) do
    local p = item:GetClipProperty("File Path")
    if type(p) == "string" and p ~= "" then
      by_path[norm(p)] = item
      by_name[norm(file_name(p))] = item
    end
  end
end
local function item_for(path)
  if not path then
    return nil
  end
  return by_path[norm(path)] or by_name[norm(file_name(path))]
end

local timeline_name = take.name .. " (live edit)"
local how = nil -- how the live edit was made
local partial = nil -- the timeline being built, until it's done
local switched_off = true

-- The live edit, laid down clip by clip through Resolve's own scripting:
-- V1 the cuts, then every angle on its own track, the sound on A1.
local function build_live_edit()
  if type(take.edit) ~= "table" or #take.edit == 0 then
    return nil, "this take has no cut list"
  end
  for _, c in ipairs(take.edit) do
    if not item_for(c.file) then
      return nil, "couldn't find " .. file_name(c.file) .. " in the media pool"
    end
  end
  pool:SetCurrentFolder(root)
  local tl = pool:CreateEmptyTimeline(timeline_name)
  if not tl then
    return nil, "Resolve couldn't make an empty timeline"
  end
  partial = tl
  project:SetCurrentTimeline(tl)
  local base = tl:GetStartFrame() or 0
  local function fail(why)
    return nil, why
  end
  -- endFrame is the last frame used (inclusive); checked on the first
  -- shot, and the other way round if this Resolve counts it differently.
  local inclusive = true
  local function place(item, from, to, at, track, media)
    local info = {
      mediaPoolItem = item,
      startFrame = from,
      endFrame = inclusive and (to - 1) or to,
      recordFrame = base + at,
      trackIndex = track,
      mediaType = media,
    }
    local got = pool:AppendToTimeline({ info })
    return got and got[1]
  end
  for i, c in ipairs(take.edit) do
    local item = item_for(c.file)
    local placed = place(item, c.from, c.to, c.at, 1, 1)
    if not placed then
      return fail("Resolve didn't take shot " .. i .. " (" .. tostring(c.scene) .. ")")
    end
    if i == 1 and c.to - c.from == c.length and placed.GetDuration then
      local d = placed:GetDuration()
      if d and d == c.length - 1 then
        inclusive = false
        tl:DeleteClips({ placed })
        if not place(item, c.from, c.to, c.at, 1, 1) then
          return fail("Resolve didn't take the first shot")
        end
      end
    end
  end
  pcall(function()
    tl:SetTrackName("video", 1, "Live edit")
  end)
  -- Every angle, full length, from the start: in parallel, in sync.
  for k, a in ipairs(take.angles or {}) do
    local item = item_for(a.file)
    if item and a.frames and a.frames > 0 then
      local track = k + 1
      while (tl:GetTrackCount("video") or 0) < track do
        if not tl:AddTrack("video") then
          break
        end
      end
      if (tl:GetTrackCount("video") or 0) >= track then
        local placed = place(item, 0, a.frames, 0, track, 1)
        if placed then
          pcall(function()
            tl:SetTrackName("video", track, a.name)
          end)
          -- Switched off, so the live edit below is what plays.
          local off = pcall(function()
            assert(tl:SetTrackEnable("video", track, false))
          end)
          if not off then
            off = pcall(function()
              assert(placed:SetClipEnabled(false))
            end)
          end
          if not off then
            switched_off = false
          end
        end
      end
    end
  end
  -- The sound under it all.
  local sound = item_for(take.sound)
  if sound then
    pcall(function()
      tl:SetTrackName("audio", 1, "Mix")
    end)
    local last = take.edit[#take.edit]
    place(sound, 0, take.frames or (last.at + last.length), 0, 1, 2)
  end
  partial = nil
  return tl
end

local timeline = nil
local ok, built, why = pcall(build_live_edit)
if ok and built then
  timeline = built
  how = "api"
else
  if not ok then
    why = tostring(built)
  end
  -- Half a timeline is no use: away with it, before the .fcpxml comes in.
  if partial then
    pcall(function()
      pool:DeleteTimelines({ partial })
    end)
    partial = nil
  end
  if take.edit then
    say("building the live edit clip by clip didn't work (" .. tostring(why) .. "); importing the .fcpxml instead.")
  end
end

-- Otherwise: the .fcpxml (Resolve brings in the live edit and every angle).
if not timeline and take.timeline then
  pool:SetCurrentFolder(root)
  timeline = pool:ImportTimelineFromFile(take.timeline, {
    timelineName = timeline_name,
    importSourceClips = false,
    sourceClipsFolders = { cameras_bin },
  })
  if not timeline then
    -- Some versions want to bring the clips in themselves.
    timeline = pool:ImportTimelineFromFile(take.timeline, { timelineName = timeline_name })
  end
  if timeline then
    how = "fcpxml"
  end
end
if timeline then
  project:SetCurrentTimeline(timeline)
  local have = timeline:GetMarkers() or {}
  local count = 0
  for _ in pairs(have) do
    count = count + 1
  end
  if count == 0 then
    for _, m in ipairs(take.markers or {}) do
      timeline:AddMarker(m.frame, "Blue", m.name, "", 1)
    end
  end
end

pm:SaveProject()
local parts = { #cameras .. " video files", #(take.audio or {}) .. " audio files" }
if linked > 0 then
  table.insert(parts, linked .. " proxies")
end
if how == "api" then
  local note = "the live edit on V1 with every camera on its own track above"
  if switched_off then
    note = note .. " (switched off: turn one on to use it)"
  else
    note = note .. " (switch tracks V2 and up off to see the live edit)"
  end
  table.insert(parts, note)
elseif how == "fcpxml" then
  table.insert(parts, "the live edit")
end
say("made the project \"" .. name .. "\" with " .. table.concat(parts, ", ") .. ".")
if not timeline then
  say("there's no live edit for this take (Cutaway saves it when Record > Recording > Editing > Editor timelines is on).")
end
