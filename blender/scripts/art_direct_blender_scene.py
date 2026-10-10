import bpy
import math
import os
import random
from mathutils import Vector


ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
SOURCE = os.path.join(ROOT, "blender", "saltline_3d_reimagining.blend")
OUTPUT = os.path.join(ROOT, "blender", "saltline_3d_artistic_pass.blend")
RENDER = os.path.join(ROOT, "blender", "renders", "saltline_3d_artistic_pass.png")

random.seed(19)
bpy.ops.wm.open_mainfile(filepath=SOURCE)


def new_material(name, color, metallic=0.0, roughness=0.65, emission=None, strength=0.0):
    material = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    material.diffuse_color = (*color, 1)
    material.use_nodes = True
    bsdf = material.node_tree.nodes.get("Principled BSDF")
    bsdf.inputs["Base Color"].default_value = (*color, 1)
    bsdf.inputs["Metallic"].default_value = metallic
    bsdf.inputs["Roughness"].default_value = roughness
    if emission:
        bsdf.inputs["Emission Color"].default_value = (*emission, 1)
        bsdf.inputs["Emission Strength"].default_value = strength
    return material


def tune_material(name, color, metallic, roughness):
    material = bpy.data.materials.get(name)
    if not material:
        return
    material.diffuse_color = (*color, 1)
    bsdf = material.node_tree.nodes.get("Principled BSDF")
    bsdf.inputs["Base Color"].default_value = (*color, 1)
    bsdf.inputs["Metallic"].default_value = metallic
    bsdf.inputs["Roughness"].default_value = roughness


def weather_material(name, dark_color, mid_color, light_color, scale=5.0,
                     roughness_low=0.45, roughness_high=0.92, bump_strength=0.18):
    """Layer broad fading, chipped islands, roughness variation, and fine pitting."""
    material = bpy.data.materials.get(name)
    if not material:
        return
    nodes = material.node_tree.nodes
    links = material.node_tree.links
    bsdf = nodes.get("Principled BSDF")

    coordinates = nodes.new("ShaderNodeTexCoord")
    coordinates.name = f"{name}_Weather_Coordinates"
    noise = nodes.new("ShaderNodeTexNoise")
    noise.name = f"{name}_Paint_Breakup"
    noise.noise_dimensions = "3D"
    noise.inputs["Scale"].default_value = scale
    noise.inputs["Detail"].default_value = 7.0
    noise.inputs["Roughness"].default_value = 0.82
    noise.inputs["Distortion"].default_value = 0.28

    color_ramp = nodes.new("ShaderNodeValToRGB")
    color_ramp.name = f"{name}_Oxidation_Ramp"
    ramp = color_ramp.color_ramp
    ramp.interpolation = "EASE"
    ramp.elements[0].position = 0.26
    ramp.elements[0].color = (*dark_color, 1)
    ramp.elements[1].position = 0.76
    ramp.elements[1].color = (*light_color, 1)
    middle = ramp.elements.new(0.5)
    middle.color = (*mid_color, 1)

    rough_ramp = nodes.new("ShaderNodeValToRGB")
    rough_ramp.name = f"{name}_Roughness_Ramp"
    rough_ramp.color_ramp.elements[0].color = (roughness_low,) * 3 + (1,)
    rough_ramp.color_ramp.elements[1].color = (roughness_high,) * 3 + (1,)

    bump = nodes.new("ShaderNodeBump")
    bump.name = f"{name}_Corrosion_Pitting"
    bump.inputs["Strength"].default_value = bump_strength
    bump.inputs["Distance"].default_value = 0.07

    links.new(coordinates.outputs["Generated"], noise.inputs["Vector"])
    links.new(noise.outputs["Fac"], color_ramp.inputs["Fac"])
    links.new(noise.outputs["Fac"], rough_ramp.inputs["Fac"])
    links.new(noise.outputs["Fac"], bump.inputs["Height"])
    links.new(color_ramp.outputs["Color"], bsdf.inputs["Base Color"])
    links.new(rough_ramp.outputs["Color"], bsdf.inputs["Roughness"])
    links.new(bump.outputs["Normal"], bsdf.inputs["Normal"])


def curve(name, points, bevel, material, cyclic=False):
    data = bpy.data.curves.new(name, "CURVE")
    data.dimensions = "3D"
    data.resolution_u = 2
    data.bevel_depth = bevel
    data.bevel_resolution = 2
    spline = data.splines.new("POLY")
    spline.points.add(len(points) - 1)
    for point, coordinate in zip(spline.points, points):
        point.co = (*coordinate, 1)
    spline.use_cyclic_u = cyclic
    obj = bpy.data.objects.new(name, data)
    bpy.context.collection.objects.link(obj)
    obj.data.materials.append(material)
    return obj


def cube(name, location, scale, material, rotation=(0, 0, 0), bevel=0.08):
    bpy.ops.mesh.primitive_cube_add(location=location, rotation=rotation)
    obj = bpy.context.object
    obj.name = name
    obj.scale = scale
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    if bevel:
        modifier = obj.modifiers.new("Art Direction Bevel", "BEVEL")
        modifier.width = bevel
        modifier.segments = 2
    obj.data.materials.append(material)
    return obj


def beacon(name, location, color_material, light_color):
    bpy.ops.mesh.primitive_uv_sphere_add(segments=20, ring_count=10, radius=0.15, location=location)
    orb = bpy.context.object
    orb.name = name
    orb.data.materials.append(color_material)
    bpy.ops.object.light_add(type="POINT", location=location)
    light = bpy.context.object
    light.name = f"{name}_Glow"
    light.data.energy = 40
    light.data.color = light_color
    light.data.shadow_soft_size = 1.2


# Modernized palette: near-black structural shadows, sun-faded oxide, cool salt,
# and a restrained cyan signal color that remains subordinate to the original orange.
tune_material("Salt White", (0.31, 0.35, 0.36), 0.0, 0.92)
tune_material("Salt Shadow", (0.055, 0.07, 0.075), 0.05, 0.82)
tune_material("Oxide", (0.48, 0.095, 0.045), 0.5, 0.48)
tune_material("Sun Bleached Oxide", (0.95, 0.23, 0.075), 0.35, 0.4)
tune_material("Deep Rust", (0.09, 0.018, 0.012), 0.65, 0.42)
tune_material("Faded Armor", (0.17, 0.26, 0.20), 0.45, 0.5)
tune_material("Sage Armor", (0.35, 0.48, 0.34), 0.34, 0.43)
tune_material("Pylon Green", (0.035, 0.075, 0.06), 0.7, 0.36)
tune_material("Chalk Marker", (0.78, 0.67, 0.45), 0.18, 0.58)
tune_material("Carbon", (0.008, 0.011, 0.012), 0.55, 0.32)

brine = new_material("Brine Cyan", (0.025, 0.19, 0.23), metallic=0.2, roughness=0.22)
basalt = new_material("Basalt", (0.018, 0.024, 0.027), metallic=0.08, roughness=0.88)
orange_glow = new_material("Signal Orange", (0.9, 0.06, 0.015), metallic=0.1,
                           roughness=0.28, emission=(1.0, 0.035, 0.005), strength=8)
cyan_glow = new_material("Signal Cyan", (0.01, 0.28, 0.38), metallic=0.1,
                         roughness=0.25, emission=(0.01, 0.55, 0.8), strength=6)
rust_run = new_material("Rust Run", (0.055, 0.008, 0.004), metallic=0.25, roughness=0.96)
salt_crust = new_material("Salt Crust", (0.68, 0.72, 0.67), metallic=0.0, roughness=1.0)

# Every inherited asset material gets non-uniform fading and corrosion. The
# patterns use generated coordinates, so they remain attached when props move.
weather_material("Oxide", (0.055, 0.008, 0.003), (0.42, 0.055, 0.022),
                 (0.72, 0.17, 0.065), scale=5.5, bump_strength=0.3)
weather_material("Sun Bleached Oxide", (0.13, 0.018, 0.006), (0.78, 0.11, 0.025),
                 (0.95, 0.40, 0.16), scale=6.5, bump_strength=0.26)
weather_material("Deep Rust", (0.012, 0.004, 0.003), (0.10, 0.015, 0.009),
                 (0.29, 0.055, 0.022), scale=4.2, bump_strength=0.34)
weather_material("Faded Armor", (0.055, 0.035, 0.022), (0.14, 0.24, 0.17),
                 (0.39, 0.42, 0.29), scale=7.5, bump_strength=0.2)
weather_material("Sage Armor", (0.08, 0.045, 0.025), (0.30, 0.43, 0.29),
                 (0.57, 0.58, 0.39), scale=8.5, bump_strength=0.22)
weather_material("Pylon Green", (0.008, 0.012, 0.01), (0.025, 0.065, 0.05),
                 (0.16, 0.20, 0.13), scale=4.8, bump_strength=0.28)
weather_material("Chalk Marker", (0.20, 0.10, 0.045), (0.67, 0.53, 0.31),
                 (0.86, 0.82, 0.65), scale=9.0, bump_strength=0.12)
weather_material("Carbon", (0.002, 0.003, 0.003), (0.018, 0.021, 0.02),
                 (0.09, 0.075, 0.06), scale=6.0, bump_strength=0.24)

# Give the world a stronger stage and a premium miniature-like silhouette.
salt_pan = bpy.data.objects.get("Salt_Pan")
if salt_pan:
    salt_pan.modifiers[0].width = 0.55
cube("Basalt_Plinth", (0, 0, -0.82), (18.5, 13.5, 0.28), basalt, bevel=0.5)

# A sinuous brine seam breaks up the grid and guides the eye from tank to silo.
brine_points = []
for index in range(17):
    x = -17 + index * 2.15
    y = -7.2 + 2.0 * math.sin(index * 0.56) + 0.12 * index
    brine_points.append((x, y, 0.055))
curve("Brine_Channel", brine_points, 0.24, brine)
curve("Brine_Highlight", [(x, y - 0.16, z + 0.018) for x, y, z in brine_points],
      0.035, cyan_glow)

# Blast crater and branching fractures anchor the collapsed asset cluster.
crater_center = Vector((1.5, -3.0, 0.08))
bpy.ops.mesh.primitive_torus_add(major_radius=2.05, minor_radius=0.25, major_segments=48,
                                 minor_segments=10, location=crater_center)
crater = bpy.context.object
crater.name = "Impact_Crater_Rim"
crater.scale.z = 0.34
crater.data.materials.append(basalt)
for branch in range(9):
    angle = branch * math.tau / 9 + random.uniform(-0.18, 0.18)
    length = random.uniform(2.3, 5.8)
    points = []
    for step in range(5):
        radius = 1.75 + length * step / 4
        wobble = math.sin(step * 2.1 + branch) * 0.25
        points.append((crater_center.x + math.cos(angle) * radius + wobble,
                       crater_center.y + math.sin(angle) * radius - wobble,
                       0.075))
    curve("Impact_Fracture", points, 0.028, basalt)

# Low-poly salt outcrops and oxide debris create readable cover and parallax.
for index in range(34):
    angle = random.random() * math.tau
    radius = random.uniform(5.0, 17.0)
    x, y = math.cos(angle) * radius, math.sin(angle) * radius * 0.68
    if abs(x) < 3 and abs(y + 3) < 3:
        continue
    bpy.ops.mesh.primitive_ico_sphere_add(subdivisions=1, radius=1,
                                         location=(x, y, random.uniform(0.08, 0.24)))
    rock = bpy.context.object
    rock.name = "Salt_Outcrop"
    rock.scale = (random.uniform(0.25, 0.9), random.uniform(0.2, 0.65),
                  random.uniform(0.12, 0.5))
    rock.rotation_euler[2] = random.random() * math.tau
    rock.data.materials.append(basalt if index % 4 == 0 else bpy.data.materials["Salt Shadow"])

# Rust drips and salt accretion are physical details, preventing the weathering
# from reading as a generic texture filter at the game camera distance.
for index, angle in enumerate((0.18, 0.72, 1.55, 2.45, 3.35, 4.25, 5.35)):
    radius = 1.39
    x = 7.8 + math.cos(angle) * radius
    y = 4.0 + math.sin(angle) * radius
    top_z = 5.0 - (index % 3) * 0.22
    bottom_z = 1.15 + (index % 4) * 0.48
    curve("Silo_Rust_Drip", [(x, y, top_z),
                              (x + math.cos(angle) * 0.035, y + math.sin(angle) * 0.035,
                               (top_z + bottom_z) / 2),
                              (x, y, bottom_z)], 0.025 + (index % 2) * 0.012, rust_run)

for index in range(24):
    segment = index / 23 * 11.0
    x = -5.0 + segment
    y = 0.8 + math.sin(segment / 1.7 * 0.8)
    angle = index * 2.4
    bpy.ops.mesh.primitive_ico_sphere_add(subdivisions=1, radius=1,
                                         location=(x, y + math.sin(angle) * 0.35,
                                                   0.5 + abs(math.cos(angle)) * 0.3))
    crust = bpy.context.object
    crust.name = "Pipeline_Salt_Accretion"
    crust.scale = (random.uniform(0.05, 0.16), random.uniform(0.035, 0.1),
                   random.uniform(0.03, 0.1))
    crust.data.materials.append(salt_crust)

# Suspended cables make the pylon read as infrastructure rather than a standalone prop.
for height, spread in ((3.8, 1.85), (5.2, 1.52), (6.4, 1.12)):
    for side in (-1, 1):
        start = Vector((9.5 + side * spread, -4.5, height))
        end = Vector((18.0 * side, -0.5, height - 0.9))
        midpoint = (start + end) / 2
        midpoint.z -= 1.3
        curve("Power_Cable", [start, midpoint, end], 0.025, basalt)

# Navigation lights add scale cues and modern game readability.
beacon("Radio_Beacon", (0.2, 6.6, 10.8), orange_glow, (1.0, 0.03, 0.01))
beacon("Silo_Status", (7.8, 4.0, 5.88), cyan_glow, (0.01, 0.45, 0.7))
beacon("Derrick_Beacon", (-7.0, 5.0, 8.0), orange_glow, (1.0, 0.03, 0.01))

# Graphic hazard markers imply gameplay lanes without turning the terrain into a UI overlay.
for index in range(7):
    cube("Hazard_Marker", (-9.5 + index * 1.05, -0.8, 0.09), (0.34, 0.08, 0.035),
         orange_glow, rotation=(0, 0, -0.15), bevel=0.025)

# Replace the flatter lighting with a cool dusk environment and a warm low sun.
for obj in list(bpy.data.objects):
    if obj.type == "LIGHT":
        bpy.data.objects.remove(obj, do_unlink=True)
bpy.ops.object.light_add(type="AREA", location=(-11, -13, 17))
sun = bpy.context.object
sun.name = "Oxide Sunset Key"
sun.data.energy = 3900
sun.data.shape = "DISK"
sun.data.size = 7
sun.data.color = (1.0, 0.24, 0.075)
sun.rotation_euler = (math.radians(24), 0, math.radians(-38))
bpy.ops.object.light_add(type="AREA", location=(10, 8, 13))
fill = bpy.context.object
fill.name = "Brine Sky Fill"
fill.data.energy = 2850
fill.data.size = 11
fill.data.color = (0.055, 0.28, 0.52)
bpy.ops.object.light_add(type="AREA", location=(0, 4, 18))
top = bpy.context.object
top.name = "Moon Top Light"
top.data.energy = 1650
top.data.size = 9
top.data.color = (0.24, 0.42, 0.58)

world = bpy.context.scene.world
background = world.node_tree.nodes.get("Background")
background.inputs["Color"].default_value = (0.003, 0.008, 0.014, 1)
background.inputs["Strength"].default_value = 0.32

# Refine the shot to a lower, more cinematic isometric angle.
camera = bpy.data.objects.get("Saltline Isometric Camera")
camera.location = (30, -36, 22)
camera.data.ortho_scale = 32
camera.data.lens = 58
target = Vector((0.5, 0.0, 2.2))
camera.rotation_euler = (target - camera.location).to_track_quat("-Z", "Y").to_euler()

scene = bpy.context.scene
scene.render.resolution_x = 1400
scene.render.resolution_y = 840
scene.render.resolution_percentage = 100
scene.render.image_settings.file_format = "PNG"
scene.render.filepath = RENDER
scene.render.engine = "BLENDER_EEVEE"
scene.render.image_settings.color_mode = "RGBA"
scene.view_settings.look = "AgX - Medium High Contrast"
scene.view_settings.exposure = 0.65

bpy.ops.wm.save_as_mainfile(filepath=OUTPUT)
bpy.ops.render.render(write_still=True)
bpy.ops.wm.save_as_mainfile(filepath=OUTPUT)
print(f"Saved artistic pass to {OUTPUT} and {RENDER}")
