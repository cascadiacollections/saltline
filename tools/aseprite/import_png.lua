-- One-time migration: wrap an exported PNG in an .aseprite source file.
--   aseprite -b --script-param src=art/sprites/x.png --script-param out=art/aseprite/x.aseprite \
--            --script-param frame=28 --script tools/aseprite/import_png.lua
-- `frame` (optional) slices a horizontal strip into frames of that width and
-- tags each one by barrel angle (0..90 in 5-degree steps).
local src = app.params["src"]
local out = app.params["out"]
local frame = tonumber(app.params["frame"])

local img = Image { fromFile = src }
local spr
if frame then
  local n = img.width // frame
  spr = Sprite(frame, img.height, ColorMode.RGB)
  for i = 0, n - 1 do
    if i > 0 then spr:newEmptyFrame() end
    local f = Image(frame, img.height, ColorMode.RGB)
    f:drawImage(img, Point(-i * frame, 0))
    spr:newCel(spr.layers[1], i + 1, f, Point(0, 0))
    local tag = spr:newTag(i + 1, i + 1)
    tag.name = string.format("%02ddeg", i * 5)
  end
else
  spr = Sprite(img.width, img.height, ColorMode.RGB)
  spr:newCel(spr.layers[1], 1, img, Point(0, 0))
end
spr.layers[1].name = "art"
spr:saveAs(out)
