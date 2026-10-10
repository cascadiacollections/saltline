import bpy
import math
import os
from mathutils import Vector


ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
BLEND_OUT = os.path.join(ROOT, "blender", "saltline_3d_reimagining.blend")
RENDER_OUT = os.path.join(ROOT, "blender", "renders", "saltline_3d_reimagining.png")


def material(name, color, metallic=0.0, roughness=0.75):
    mat = bpy.data.materials.new(name)
    mat.diffuse_color = (*color, 1.0)
    mat.use_nodes = True
    bsdf = mat.node_tree.nodes.get("Principled BSDF")
    bsdf.inputs["Base Color"].default_value = (*color, 1.0)
    bsdf.inputs["Metallic"].default_value = metallic
    bsdf.inputs["Roughness"].default_value = roughness
    return mat


def apply_mat(obj, mat):
    obj.data.materials.append(mat)
    return obj


def cube(name, location, scale, mat, bevel=0.08, rotation=(0, 0, 0)):
    bpy.ops.mesh.primitive_cube_add(location=location, rotation=rotation)
    obj = bpy.context.object
    obj.name = name
    obj.scale = scale
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    if bevel:
        mod = obj.modifiers.new("Soft industrial edges", "BEVEL")
        mod.width = bevel
        mod.segments = 2
    return apply_mat(obj, mat)


def cylinder(name, location, radius, depth, mat, vertices=24, rotation=(0, 0, 0)):
    bpy.ops.mesh.primitive_cylinder_add(
        vertices=vertices, radius=radius, depth=depth, location=location, rotation=rotation
    )
    obj = bpy.context.object
    obj.name = name
    return apply_mat(obj, mat)


def beam(name, start, end, width, mat):
    start, end = Vector(start), Vector(end)
    midpoint = (start + end) / 2
    direction = end - start
    obj = cube(name, midpoint, (width, width, direction.length / 2), mat, bevel=width * 0.3)
    obj.rotation_mode = "QUATERNION"
    obj.rotation_quaternion = direction.to_track_quat("Z", "Y")
    return obj


def ring(name, location, major, minor, mat, rotation=(0, 0, 0)):
    bpy.ops.mesh.primitive_torus_add(
        major_radius=major, minor_radius=minor, major_segments=32, minor_segments=8,
        location=location, rotation=rotation
    )
    obj = bpy.context.object
    obj.name = name
    return apply_mat(obj, mat)


def armored_prism(name, rear_x, front_x, rear_width, front_width,
                  bottom_z, rear_top_z, front_top_z, mat, bevel=0.06):
    """Angular eight-vertex armored volume with a sloped roof and plan taper."""
    vertices = [
        (rear_x, -rear_width, bottom_z), (rear_x, rear_width, bottom_z),
        (front_x, -front_width, bottom_z), (front_x, front_width, bottom_z),
        (rear_x, -rear_width, rear_top_z), (rear_x, rear_width, rear_top_z),
        (front_x, -front_width, front_top_z), (front_x, front_width, front_top_z),
    ]
    faces = [
        (0, 2, 3, 1), (4, 5, 7, 6), (0, 4, 6, 2),
        (1, 3, 7, 5), (0, 1, 5, 4), (2, 6, 7, 3),
    ]
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(vertices, [], faces)
    mesh.update()
    obj = bpy.data.objects.new(name, mesh)
    bpy.context.collection.objects.link(obj)
    if bevel:
        mod = obj.modifiers.new("Armor edge roll", "BEVEL")
        mod.width = bevel
        mod.segments = 2
    return apply_mat(obj, mat)


def make_tank(origin, angle=-0.25):
    root = bpy.data.objects.new("Tank_Abrams_Inspired_Root", None)
    bpy.context.collection.objects.link(root)
    root.location = origin
    root.rotation_euler[2] = angle

    # Seven-wheel running gear and continuous track blocks create the long,
    # low silhouette of a contemporary main battle tank.
    for side in (-1, 1):
        track = cube("Abrams_Track_Bed", (0, side * 1.12, 0.48), (2.75, 0.34, 0.47), dark, 0.2)
        track.parent = root
        for wx in (-2.15, -1.45, -0.72, 0.0, 0.72, 1.45, 2.15):
            wheel = cylinder("Abrams_Road_Wheel", (wx, side * 1.47, 0.48), 0.31, 0.13,
                             rust_dark, 20,
                             rotation=(math.pi / 2, 0, 0))
            wheel.parent = root
            hub = cylinder("Road_Wheel_Hub", (wx, side * 1.55, 0.48), 0.13, 0.05,
                           cream, 16, rotation=(math.pi / 2, 0, 0))
            hub.parent = root
        for tx in (-2.45, -1.95, -1.45, -0.95, -0.45, 0.05, 0.55, 1.05, 1.55, 2.05, 2.55):
            for tz in (0.05, 0.91):
                tread = cube("Individual_Track_Pad", (tx, side * 1.14, tz),
                             (0.21, 0.39, 0.065), dark, 0.025)
                tread.parent = root

        # Modular side skirts echo the original sprite's readable horizontal bands.
        for panel_index, px in enumerate((-2.25, -1.5, -0.75, 0, 0.75, 1.5, 2.25)):
            skirt = cube("Composite_Side_Skirt", (px, side * 1.47, 1.02),
                         (0.34, 0.055, 0.38), green if panel_index < 5 else green_dark, 0.045)
            skirt.parent = root

    lower_hull = armored_prism("Abrams_Lower_Hull", -2.65, 2.65, 1.28, 1.02,
                               0.72, 1.38, 1.52, green, 0.12)
    lower_hull.parent = root
    glacis = armored_prism("Abrams_Glacis", -0.4, 2.72, 1.18, 0.82,
                           1.28, 1.67, 1.78, green_light, 0.1)
    glacis.parent = root

    # Broad, faceted composite turret cheeks with a long rear ammunition bustle.
    turret = armored_prism("Abrams_Faceted_Turret", -1.42, 1.22, 1.18, 0.76,
                           1.58, 2.46, 2.20, green_light, 0.1)
    turret.parent = root
    bustle = cube("Turret_Bustle", (-1.75, 0, 2.02), (0.55, 1.12, 0.4), green_dark, 0.11)
    bustle.parent = root
    for side in (-1, 1):
        rail_top = beam("Bustle_Rack_Rail", (-2.35, side * 1.18, 1.75),
                        (-1.18, side * 1.18, 1.75), 0.035, dark)
        rail_top.parent = root
        for px in (-2.2, -1.85, -1.5):
            rack = beam("Bustle_Rack_Vertical", (px, side * 1.18, 1.72),
                        (px, side * 1.18, 2.35), 0.028, dark)
            rack.parent = root

    # Mantlet, thermal sight, commander's cupola, and remote weapon station.
    mantlet = cube("Gun_Mantlet", (1.15, 0, 2.08), (0.25, 0.5, 0.34), dark, 0.09)
    mantlet.parent = root
    sight = cube("Thermal_Optic", (0.12, -0.62, 2.55), (0.24, 0.2, 0.24), dark, 0.06)
    sight.parent = root
    lens = cube("Thermal_Lens", (0.28, -0.625, 2.58), (0.035, 0.13, 0.12), cream, 0.02)
    lens.parent = root
    cupola = cylinder("Commander_Cupola", (-0.62, 0.45, 2.55), 0.34, 0.16, dark, 24)
    cupola.parent = root
    hatch = cylinder("Commander_Hatch", (-0.62, 0.45, 2.66), 0.29, 0.07, green, 24)
    hatch.parent = root
    weapon_mount = cube("Remote_Weapon_Mount", (-0.45, -0.32, 2.83),
                        (0.18, 0.13, 0.18), dark, 0.04)
    weapon_mount.parent = root
    mg = cylinder("Remote_Weapon_Barrel", (0.18, -0.32, 2.88), 0.045, 1.15,
                  dark, 12, rotation=(0, math.pi / 2, 0))
    mg.parent = root

    # Long smoothbore main gun with a thicker bore evacuator.
    barrel = cylinder("120mm_Smoothbore", (3.5, 0, 2.18), 0.105, 5.05, rust, 20,
                      rotation=(0, math.pi / 2 - 0.035, 0))
    barrel.parent = root
    evacuator = cylinder("Bore_Evacuator", (2.75, 0, 2.20), 0.18, 0.72, rust_dark, 20,
                         rotation=(0, math.pi / 2 - 0.035, 0))
    evacuator.parent = root
    muzzle = cylinder("Main_Gun_Muzzle", (6.03, 0, 2.27), 0.155, 0.28, dark, 20,
                      rotation=(0, math.pi / 2 - 0.035, 0))
    muzzle.parent = root

    # Twin smoke-grenade banks reinforce the turret cheek silhouette.
    for side in (-1, 1):
        for index in range(4):
            smoke = cylinder("Smoke_Grenade_Launcher",
                             (0.58 - index * 0.18, side * (0.86 + index * 0.035), 2.34),
                             0.07, 0.32, dark, 12,
                             rotation=(0.62, 0, -side * 0.22))
            smoke.parent = root

    antenna = beam("Tank_Long_Antenna", (-1.55, 0.76, 2.36),
                   (-1.85, 0.82, 4.0), 0.018, cream)
    antenna.parent = root
    antenna_2 = beam("Tank_Command_Antenna", (-1.48, -0.7, 2.35),
                     (-1.62, -0.74, 3.55), 0.016, dark)
    antenna_2.parent = root

    # Headlamps and towing eyes remain oversized enough for the game's camera distance.
    for side in (-1, 1):
        lamp = cube("Tank_Headlamp", (2.55, side * 0.68, 1.55),
                    (0.09, 0.14, 0.1), cream, 0.025)
        lamp.parent = root
        tow = ring("Tow_Eye", (2.6, side * 0.78, 1.0), 0.12, 0.028, dark,
                   rotation=(math.pi / 2, 0, 0))
        tow.parent = root
    return root


def make_silo(origin):
    x, y, z = origin
    cylinder("Silo", (x, y, z + 2.4), 1.35, 4.8, rust, 32)
    bpy.ops.mesh.primitive_cone_add(vertices=32, radius1=1.42, radius2=0.18, depth=1.0,
                                    location=(x, y, z + 5.3))
    apply_mat(bpy.context.object, rust_light)
    for level in (0.8, 2.0, 3.2, 4.4):
        ring("Silo_Rib", (x, y, z + level), 1.37, 0.055, rust_dark)
    beam("Silo_Ladder", (x + 1.42, y, z + 0.2), (x + 1.42, y, z + 5.5), 0.055, cream)
    for level in [0.55 + i * 0.45 for i in range(11)]:
        beam("Ladder_Rung", (x + 1.47, y - 0.28, z + level),
             (x + 1.47, y + 0.28, z + level), 0.035, cream)


def lattice_tower(name, origin, height, base, top, mat):
    x, y, z = origin
    corners0 = [(x + sx * base, y + sy * base, z) for sx in (-1, 1) for sy in (-1, 1)]
    corners1 = [(x + sx * top, y + sy * top, z + height) for sx in (-1, 1) for sy in (-1, 1)]
    for i in range(4):
        beam(f"{name}_Leg", corners0[i], corners1[i], 0.09, mat)
    steps = 5
    for step in range(steps):
        t0, t1 = step / steps, (step + 1) / steps
        zz0, zz1 = z + height * t0, z + height * t1
        r0 = base + (top - base) * t0
        r1 = base + (top - base) * t1
        for sy in (-1, 1):
            beam(f"{name}_Brace", (x - r0, y + sy * r0, zz0),
                 (x + r1, y + sy * r1, zz1), 0.045, mat)
            beam(f"{name}_Brace", (x + r0, y + sy * r0, zz0),
                 (x - r1, y + sy * r1, zz1), 0.045, mat)
        for sx in (-1, 1):
            beam(f"{name}_Brace", (x + sx * r0, y - r0, zz0),
                 (x + sx * r1, y + r1, zz1), 0.045, mat)
    return (x, y, z + height)


def make_derrick(origin):
    top = lattice_tower("Derrick", origin, 7.5, 1.5, 0.42, rust_dark)
    x, y, z = top
    cube("Derrick_Platform", (x, y, z - 0.15), (0.85, 0.85, 0.12), rust, 0.05)
    beam("Derrick_Crown", (x - 0.8, y, z + 0.2), (x + 0.8, y, z + 0.2), 0.11, cream)
    beam("Drill_String", (x, y, z - 0.2), (x, y, 0.15), 0.035, dark)


def make_radio_mast(origin):
    top = lattice_tower("Radio", origin, 9.0, 0.75, 0.12, cream)
    x, y, z = top
    beam("Radio_Spire", (x, y, z), (x, y, z + 1.8), 0.06, rust_light)
    for dz in (4.3, 6.2, 8.0):
        ring("Radio_Hoop", (x, y, origin[2] + dz), 0.42, 0.055, rust,
             rotation=(math.pi / 2, 0, 0))


def make_pylon(origin):
    x, y, z = origin
    lattice_tower("Pylon", origin, 6.5, 1.1, 0.25, green_dark)
    for zz, span in ((z + 3.8, 2.4), (z + 5.2, 2.0), (z + 6.4, 1.5)):
        beam("Pylon_Arm", (x - span, y, zz), (x + span, y, zz), 0.1, green)
        for side in (-1, 1):
            beam("Insulator", (x + side * span * 0.78, y, zz),
                 (x + side * span * 0.78, y, zz - 0.5), 0.06, cream)


def make_wreckage(origin):
    x, y, z = origin
    for i in range(7):
        a = i * 0.83
        length = 1.2 + (i % 3) * 0.45
        beam("Collapsed_Structure", (x + math.cos(a) * i * 0.32, y + math.sin(a) * i * 0.24, z + 0.1),
             (x + math.cos(a) * i * 0.32 + math.cos(a + 0.7) * length,
              y + math.sin(a) * i * 0.24 + math.sin(a + 0.7) * length,
              z + 0.2 + (i % 2) * 0.35), 0.09, rust_dark)
    for i in range(10):
        cube("Debris", (x - 2 + (i % 5) * 0.75, y - 0.8 + (i // 5) * 0.8, z + 0.12),
             (0.22 + 0.05 * (i % 2), 0.14, 0.1), rust, 0.04, rotation=(0, 0, i * 0.43))


# Clean scene and establish the restrained sprite-inspired palette.
bpy.ops.object.select_all(action="SELECT")
bpy.ops.object.delete(use_global=False)
for datablock in (bpy.data.materials, bpy.data.curves, bpy.data.meshes):
    for item in list(datablock):
        if item.users == 0:
            datablock.remove(item)

salt = material("Salt White", (0.74, 0.69, 0.58), roughness=0.95)
salt_dark = material("Salt Shadow", (0.32, 0.31, 0.28), roughness=1.0)
rust = material("Oxide", (0.55, 0.18, 0.10), metallic=0.35)
rust_light = material("Sun Bleached Oxide", (0.82, 0.35, 0.20), metallic=0.2)
rust_dark = material("Deep Rust", (0.22, 0.055, 0.035), metallic=0.5)
green = material("Faded Armor", (0.24, 0.32, 0.22), metallic=0.25)
green_light = material("Sage Armor", (0.40, 0.48, 0.32), metallic=0.2)
green_dark = material("Pylon Green", (0.10, 0.16, 0.12), metallic=0.55)
cream = material("Chalk Marker", (0.84, 0.70, 0.47), metallic=0.1)
dark = material("Carbon", (0.025, 0.022, 0.02), metallic=0.3)

# Salt pan, dried channels, and raised strata.
cube("Salt_Pan", (0, 0, -0.38), (18, 13, 0.38), salt, 0.35)
for i in range(17):
    x = -15 + i * 1.9
    beam("Salt_Fracture", (x, -11.5 + (i % 3), 0.01),
         (x + 2.8 * math.sin(i), 10.5 - (i % 4), 0.01), 0.018, salt_dark)
for i in range(5):
    cube("Salt_Ridge", (-10 + i * 4.8, 8.2 + math.sin(i) * 0.8, 0.12),
         (1.7, 0.42, 0.14), salt_dark, 0.22, rotation=(0, 0, 0.25 * math.sin(i)))

# Recompose the sprite silhouettes as a navigable 3D industrial battlefield.
make_tank((-7.0, -4.6, 0.0), angle=-0.22)
make_silo((7.8, 4.0, 0.0))
make_derrick((-7.0, 5.0, 0.0))
make_radio_mast((0.2, 6.6, 0.0))
make_pylon((9.5, -4.5, 0.0))
make_wreckage((1.5, -3.0, 0.0))

# A partially buried pipeline threads the composition together.
for i in range(7):
    cylinder("Pipeline", (-5.0 + i * 1.7, 0.8 + math.sin(i * 0.8), 0.46), 0.34, 1.8,
             rust_light, 20, rotation=(0, math.pi / 2, 0.18 * math.cos(i)))
    ring("Pipeline_Joint", (-5.8 + i * 1.7, 0.8 + math.sin(i * 0.8), 0.46),
         0.37, 0.055, rust_dark, rotation=(0, math.pi / 2, 0))

# Lighting and cinematic orthographic camera.
bpy.ops.object.light_add(type="AREA", location=(-5, -7, 15))
key = bpy.context.object
key.name = "Low Amber Sun"
key.data.energy = 1800
key.data.shape = "DISK"
key.data.size = 8
key.data.color = (1.0, 0.48, 0.25)
key.rotation_euler = (math.radians(23), 0, math.radians(-28))

bpy.ops.object.light_add(type="AREA", location=(8, 5, 10))
fill = bpy.context.object
fill.name = "Cold Sky Fill"
fill.data.energy = 900
fill.data.size = 12
fill.data.color = (0.34, 0.48, 0.62)

bpy.ops.object.camera_add(location=(26, -31, 25))
camera = bpy.context.object
camera.name = "Saltline Isometric Camera"
camera.data.type = "ORTHO"
camera.data.ortho_scale = 34
target = Vector((0, 0, 2.5))
camera.rotation_euler = (target - camera.location).to_track_quat("-Z", "Y").to_euler()
bpy.context.scene.camera = camera

world = bpy.context.scene.world
world.use_nodes = True
world.node_tree.nodes["Background"].inputs["Color"].default_value = (0.018, 0.025, 0.034, 1)
world.node_tree.nodes["Background"].inputs["Strength"].default_value = 0.28

scene = bpy.context.scene
scene.render.engine = "BLENDER_EEVEE"
scene.render.resolution_x = 1200
scene.render.resolution_y = 760
scene.render.resolution_percentage = 100
scene.render.image_settings.file_format = "PNG"
scene.render.filepath = RENDER_OUT
scene.render.film_transparent = False
scene.view_settings.look = "AgX - Medium High Contrast"

# Ground-level mist adds depth while preserving the graphic silhouette language.
scene.render.image_settings.color_mode = "RGBA"
bpy.ops.wm.save_as_mainfile(filepath=BLEND_OUT)
bpy.ops.render.render(write_still=True)
print(f"Saved {BLEND_OUT} and rendered {RENDER_OUT}")
