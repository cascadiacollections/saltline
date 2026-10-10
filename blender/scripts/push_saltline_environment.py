"""Second art-direction pass: mineral landscape and industrial storytelling."""
import bpy, math, random, os
from mathutils import Vector
R = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
random.seed(81)
bpy.ops.wm.open_mainfile(filepath=os.path.join(R, 'blender', 'saltline_3d_artistic_pass.blend'))

def mat(name, color, metal=0, rough=.8):
    m=bpy.data.materials.new(name); m.diffuse_color=(*color,1); m.use_nodes=True
    p=m.node_tree.nodes.get('Principled BSDF'); p.inputs['Base Color'].default_value=(*color,1)
    p.inputs['Metallic'].default_value=metal; p.inputs['Roughness'].default_value=rough
    return m

chalk=[mat('Mineral crust %02d'%i,(.40+i*.022,.43+i*.021,.40+i*.02)) for i in range(7)]
earth=mat('Exposed dark sediment',(.085,.10,.09))
water=mat('Still saline water',(.035,.16,.16),.45,.16)
iron=bpy.data.materials['Deep Rust']; paint=bpy.data.materials['Sun Bleached Oxide']

def mesh(name, verts, faces, material):
    d=bpy.data.meshes.new(name); d.from_pydata(verts,[],faces); d.update()
    o=bpy.data.objects.new(name,d); bpy.context.collection.objects.link(o); d.materials.append(material); return o

def box(name,loc,size,material):
    bpy.ops.mesh.primitive_cube_add(size=1,location=loc); o=bpy.context.object; o.name=name; o.scale=size
    bpy.ops.object.transform_apply(location=False,rotation=False,scale=True)
    o.data.materials.append(material); b=o.modifiers.new('Worn edges','BEVEL'); b.width=.025; b.segments=2
    return o

def cable(name,points,r=.02,material=iron):
    c=bpy.data.curves.new(name,'CURVE'); c.dimensions='3D'; c.bevel_depth=r; c.bevel_resolution=2
    s=c.splines.new('POLY'); s.points.add(len(points)-1)
    for p,v in zip(s.points,points): p.co=(*v,1)
    o=bpy.data.objects.new(name,c); bpy.context.collection.objects.link(o); c.materials.append(material); return o

# Replace decorative straight cracks, pipe-shaped water, and plinth with actual
# separated mineral plates. Existing modeled props and weathering stay intact.
prefixes=('Salt_Fracture','Salt_Ridge','Brine_Channel','Brine_Highlight','Basalt_Plinth',
          'Impact_Fracture','Impact_Crater_Rim','Power_Cable','Hazard_Marker','Silo_Rust_Drip')
for o in list(bpy.data.objects):
    if o.name.startswith(prefixes): bpy.data.objects.remove(o,do_unlink=True)
pan=bpy.data.objects.get('Salt_Pan'); pan.data.materials.clear(); pan.data.materials.append(earth)
pan.location.z=-.65

# Clip a jittered Voronoi tessellation using half-planes, without dependencies.
seeds=[(-17+x*1.6+random.uniform(-.5,.5),-12+y*1.6+random.uniform(-.5,.5)) for x in range(22) for y in range(16)]
def clip(poly,a,b,c):
    out=[]
    for p,q in zip(poly,poly[1:]+poly[:1]):
        u=a*p[0]+b*p[1]-c; v=a*q[0]+b*q[1]-c
        if u<=0: out.append(p)
        if (u<=0)!=(v<=0):
            t=u/(u-v); out.append((p[0]+t*(q[0]-p[0]),p[1]+t*(q[1]-p[1])))
    return out

for i,(x,y) in enumerate(seeds):
    # Shallow crescent basin in foreground, away from the tank.
    if ((x-3)/7)**2+((y+8)/2.4)**2 < 1: continue
    poly=[(-18,-13),(18,-13),(18,13),(-18,13)]
    for xx,yy in seeds:
        if (xx==x and yy==y) or abs(xx-x)>4 or abs(yy-y)>4: continue
        poly=clip(poly,xx-x,yy-y,(xx*xx+yy*yy-x*x-y*y)/2)
        if not poly: break
    if len(poly)<3: continue
    poly=[(x+(px-x)*.965,y+(py-y)*.965) for px,py in poly]
    h=random.uniform(-.055,.018); n=len(poly)
    verts=[(px,py,h) for px,py in poly]+[(px,py,-.27) for px,py in poly]
    faces=[tuple(range(n)),tuple(reversed(range(n,2*n)))]+[(j,(j+1)%n,(j+1)%n+n,j+n) for j in range(n)]
    mesh('Fractured salt plate',verts,faces,random.choice(chalk))

box('Shallow brine surface',(3,-8,-.20),(14.5,5.4,.025),water)

# Concrete footings ground each tower in the landscape.
for x,y,w in [(-7,5,1.5),(.2,6.6,.75),(9.5,-4.5,1.1)]:
    for sx in (-1,1):
        for sy in (-1,1): box('Cast foundation',(x+sx*w,y+sy*w,.10),(.55,.55,.24),chalk[1])

# A small abandoned pump station supplies narrative and intermediate scale.
box('Pump house',(3.9,3.5,.9),(2.6,1.8,1.8),bpy.data.materials['Faded Armor'])
box('Oxide sheet roof',(3.9,3.5,1.86),(2.9,2.1,.14),paint)
for i in range(12): box('Roof seams',(2.6+i*.23,3.5,1.96),(.035,2.08,.045),iron)
box('Service door',(3.9,2.575,.75),(.85,.04,1.45),iron)
for x in (3.15,4.65): box('Vent housing',(x,2.56,1.3),(.42,.05,.4),iron)
for i in range(6): box('Vent louvres',(3.15,2.52,1.14+i*.06),(.37,.025,.018),chalk[2])

# Smooth sagging utility lines terminate on the ground instead of in midair.
for side in (-1,1):
    start=Vector((9.5+side*1.5,-4.5,5.2)); end=Vector((15,8,.15))
    cable('Abandoned sagging line',[tuple(start.lerp(end,t/32)+Vector((0,0,-1.7*4*(t/32)*(1-t/32)))) for t in range(33)])

# Track impressions behind the tank, aligned with its current heading.
root=bpy.data.objects.get('Tank_Abrams_Inspired_Root')
for side in (-1,1):
    for i in range(20):
        p=root.matrix_world @ Vector((-2.8-i*.27,side*1.12,-.003))
        o=box('Pressed tread in salt',p,(.13,.62,.012),earth); o.rotation_euler.z=root.rotation_euler.z

# Surface-level corrosion streaks on the cylinder follow gravity.
for i in range(28):
    a=random.random()*math.tau; z=random.choice((2,3.2,4.4)); length=random.uniform(.15,.75)
    x=7.8+1.352*math.cos(a); y=4+1.352*math.sin(a)
    cable('Thin oxide runoff',[(x,y,z),(x,y,z-length)],random.uniform(.008,.018),iron)

# Softer, less saturated illumination lets the sage and weathering remain visible.
for o in bpy.data.objects:
    if o.type=='LIGHT':
        if 'Sunset' in o.name: o.data.color=(1,.67,.4); o.data.energy=4300
        if 'Sky Fill' in o.name: o.data.color=(.42,.66,.8); o.data.energy=3200
        if 'Moon' in o.name: o.data.color=(.7,.8,1); o.data.energy=2000
scene=bpy.context.scene; scene.view_settings.exposure=.5
scene.world.node_tree.nodes['Background'].inputs['Color'].default_value=(.07,.11,.14,1)
scene.world.node_tree.nodes['Background'].inputs['Strength'].default_value=.45
cam=scene.camera; cam.location=(32,-40,31); cam.data.ortho_scale=43
cam.rotation_euler=(Vector((0,0,2))-cam.location).to_track_quat('-Z','Y').to_euler()
scene.render.resolution_x=1600; scene.render.resolution_y=1100
scene.render.filepath=os.path.join(R,'blender','renders','saltline_environment_v2.png')
for a in bpy.context.screen.areas:
    if a.type=='VIEW_3D': a.spaces.active.region_3d.view_perspective='CAMERA'
bpy.ops.wm.save_as_mainfile(filepath=os.path.join(R,'blender','saltline_environment_v2.blend'))
bpy.ops.render.render(write_still=True)
