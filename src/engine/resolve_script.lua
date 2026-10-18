-- Cutaway - Build project from a take
--
-- In DaVinci Resolve: Workspace > Scripts > Cutaway - Build project from a take.
-- Pick a take folder recorded by Cutaway (it starts at the latest take).
-- You get a new project named after the take, at the take's frame rate,
-- with bins for the cameras, the program and the audio, proxies linked,
-- and the live edit as a timeline (with the markers).
--
-- Installed by Cutaway (Settings > Recording). Works in the free Resolve.

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

-- Cutaway notes the latest take here.
local function latest_take()
  local candidates = {}
  local home = os.getenv("HOME")
  local appdata = os.getenv("APPDATA")
  if home then
    table.insert(candidates, home .. "/Library/Application Support/Cutaway/last-take.txt")
    table.insert(candidates, home .. "/.config/cutaway/last-take.txt")
  end
  if appdata then
    table.insert(candidates, appdata .. "\\Cutaway\\last-take.txt")
  end
  for _, path in ipairs(candidates) do
    local f = io.open(path, "r")
    if f then
      local folder = f:read("*l")
      f:close()
      if folder and folder ~= "" then
        return folder
      end
    end
  end
  return nil
end

local function take_file(folder)
  for _, entry in ipairs(bmd.readdir(folder .. sep .. "*(Resolve).lua") or {}) do
    if not entry.IsDir then
      return folder .. sep .. entry.Name
    end
  end
  return nil
end

local start = latest_take()
local folder = fusion:RequestDir(start or "")
if not folder or folder == "" then
  say("no folder chosen.")
  return
end
folder = folder:gsub("[/\\]+$", "")
local file = take_file(folder)
if not file then
  say("that folder isn't a Cutaway take (there's no \"(Resolve).lua\" file in it).")
  return
end
local ok, take = pcall(dofile, file)
if not ok or type(take) ~= "table" then
  say("couldn't read " .. file)
  return
end

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
local fps = tostring(take.fps)
project:SetSetting("timelineFrameRate", fps)
project:SetSetting("timelinePlaybackFrameRate", fps)

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
bin("Audio", take.audio or {})

-- Proxies (small copies) for smooth editing.
local linked = 0
for _, item in ipairs(cameras) do
  local path = item:GetClipProperty("File Path")
  local proxy = take.proxies and take.proxies[path]
  if proxy and item:LinkProxyMedia(proxy) then
    linked = linked + 1
  end
end

-- The live edit.
local timeline = nil
if take.timeline then
  pool:SetCurrentFolder(root)
  timeline = pool:ImportTimelineFromFile(take.timeline, {
    timelineName = take.name .. " (live edit)",
    importSourceClips = false,
    sourceClipsFolders = { cameras_bin },
  })
  if not timeline then
    -- Some versions want to bring the clips in themselves.
    timeline = pool:ImportTimelineFromFile(take.timeline, { timelineName = take.name .. " (live edit)" })
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
if timeline then
  table.insert(parts, "the live edit")
end
say("made the project \"" .. name .. "\" with " .. table.concat(parts, ", ") .. ".")
