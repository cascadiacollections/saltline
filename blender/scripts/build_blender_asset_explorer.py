import bpy
import glob
import os


ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "art", "sprites"))
OUTPUT = os.path.abspath(
    os.path.join(os.path.dirname(__file__), "..", "saltline_sprite_explorer.blend")
)

bpy.ops.object.select_all(action="SELECT")
bpy.ops.object.delete(use_global=False)

files = sorted(glob.glob(os.path.join(ROOT, "*.png")))
columns = 6
column_spacing = 4.2
row_spacing = 3.6

for index, path in enumerate(files):
    row, column = divmod(index, columns)
    x = (column - (columns - 1) / 2) * column_spacing
    y = -row * row_spacing

    image = bpy.data.images.load(path, check_existing=True)
    bpy.ops.object.empty_add(type="IMAGE", location=(x, y, 0))
    sprite = bpy.context.object
    sprite.name = os.path.splitext(os.path.basename(path))[0]
    sprite.data = image
    sprite.empty_display_size = 3.0
    sprite.empty_image_depth = "FRONT"
    sprite.color[3] = 1.0

    bpy.ops.object.text_add(location=(x, y - 1.65, 0.05))
    label = bpy.context.object
    label.name = f"Label_{sprite.name}"
    label.data.body = sprite.name.replace("_", " ")
    label.data.align_x = "CENTER"
    label.data.size = 0.34
    label.data.extrude = 0.01

bpy.ops.object.text_add(location=(0, 2.7, 0.05))
title = bpy.context.object
title.name = "Title"
title.data.body = "SALTLINE - 2D ASSET EXPLORER"
title.data.align_x = "CENTER"
title.data.size = 0.8

bpy.context.scene.world.color = (0.025, 0.025, 0.025)
bpy.ops.wm.save_as_mainfile(filepath=OUTPUT)
print(f"Created {len(files)} assets in {OUTPUT}")
