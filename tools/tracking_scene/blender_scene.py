"""Blender >=4 headless scene authoring. Run with blender -b --python ... -- COMMAND.

Cameras follow OpenCV: pinhole intrinsics with skew and Brown-Conrady distortion
(k1, k2, p1, p2, k3). Blender renders an ideal pinhole view with overscan; each
output pixel is then remapped through the inverse distortion. A nonzero exposure
averages EXPOSURE_SAMPLES linear-light renders across the exposure interval, with
the camera pose interpolated between authored frames (global shutter; no rolling
shutter). Images are ideal emissive geometry, not radiometric/sensor simulation.
shortcut: planar UVs approximate homography within 1/4096 normalized UV using
65..256 tessellation; extreme fits reject until projective shaders are needed.
Edited planes export as grids with original homography retained as baseline.
"""
import argparse
import copy
import hashlib
import json
import math
from pathlib import Path
import subprocess
import sys
import tempfile

import bpy
import numpy as np
from mathutils import Matrix, Quaternion, Vector

METADATA = "odyssey_scene.json"
PLANAR_RESOLUTION = 65
CV_TO_BLENDER = Matrix.Diagonal((1.0, -1.0, -1.0, 1.0))
EXPOSURE_SAMPLES = 16
MAX_OVERSCAN = 4


def require(condition, message):
    if not condition:
        raise ValueError(message)


def quaternion(values):
    require(len(values) == 4 and all(math.isfinite(v) for v in values), "invalid quaternion")
    require(abs(sum(v * v for v in values) - 1) < 1e-4, "nonunit quaternion")
    x, y, z, w = values
    return Quaternion((w, x, y, z))


def homography(calibration):
    values = calibration["homography"]
    require(len(values) == 9 and all(math.isfinite(v) for v in values), "invalid homography")
    return Matrix([[values[c * 3 + r] for c in range(3)] for r in range(3)])


def plane_point(inverse, u, v):
    point = inverse @ Vector((u, v, 1))
    require(abs(point.z) > 1e-12, "projective horizon touches screen")
    return (point.x / point.z, point.y / point.z, 0.0)


def surface_geometry(screen):
    surface = screen["calibration"].get("surface")
    if surface is None:
        transform = homography(screen["calibration"])
        inverse = transform.inverted()
        signs = [((inverse @ Vector((u, v, 1))).z > 0) for u, v in ((0, 0), (1, 0), (1, 1), (0, 1))]
        require(len(set(signs)) == 1, "projective horizon crosses screen")
        for resolution in (PLANAR_RESOLUTION, 129, 256):
            columns = rows = resolution
            points = [plane_point(inverse, x / (columns - 1), y / (rows - 1))
                      for y in range(rows) for x in range(columns)]
            weights = [abs((transform @ Vector((point[0], point[1], 1))).z) for point in points]
            maximum_error = 0.0
            for y in range(rows - 1):
                for x in range(columns - 1):
                    tl = y * columns + x
                    values = [weights[index] for index in (tl, tl + 1, tl + columns, tl + columns + 1)]
                    # Rational-vs-linear interpolation error <= UV span * relative weight spread.
                    maximum_error = max(maximum_error, (max(values) / min(values) - 1) / (resolution - 1))
            if maximum_error <= 1 / 4096:
                break
        require(maximum_error <= 1 / 4096, "projective fit exceeds bounded UV tessellation accuracy")
    else:
        columns, rows = surface["columns"], surface["rows"]
        require(2 <= columns <= 256 and 2 <= rows <= 256, "grid dimensions outside 2..256")
        points = surface["positions_m"]
        require(len(points) == columns * rows, "grid position count mismatch")
    require(all(len(p) == 3 and all(math.isfinite(v) for v in p) for p in points), "invalid screen positions")
    triangles = []
    for y in range(rows - 1):
        for x in range(columns - 1):
            tl = y * columns + x
            triangles.extend(((tl, tl + 1, tl + columns + 1), (tl, tl + columns + 1, tl + columns)))
    return columns, rows, points, triangles


def validate(scene, validator):
    # Rust owns complete geometric invariants, including folds and boundary intersections.
    with tempfile.TemporaryDirectory(prefix="odyssey-scene-validation-") as directory:
        path = Path(directory) / "scene.json"
        path.write_text(json.dumps(scene, allow_nan=False))
        try:
            subprocess.run([validator, "validate-scene", str(path)], check=True)
        except FileNotFoundError as error:
            raise ValueError(f"Rust validator unavailable: {validator}; build tracking-regression first") from error
    require(scene["schema_version"] == 1, "requires scene schema 1")
    require(scene["coordinate_basis"] == "screen_x_right_y_down_z_forward", "unsupported coordinate basis")


def check_render_support(scene):
    for camera in scene["cameras"]:
        k = camera["intrinsics"]
        require(k[1][0] == 0 and k[2] == [0, 0, 1],
                f"camera {camera['id']}: intrinsics must be an OpenCV upper-triangular matrix")
    for frame in scene["frames"]:
        if frame["exposure_us"]:
            track = camera_track(scene, frame["camera_id"])
            require(track[-1]["elapsed_us"] >= frame["elapsed_us"] + frame["exposure_us"],
                    f"frame {frame['id']}: exposure extends past the camera's last authored pose")


def camera_track(scene, camera_id):
    return [frame for frame in scene["frames"] if frame["camera_id"] == camera_id]


def camera_of(scene, frame):
    return next(camera for camera in scene["cameras"] if camera["id"] == frame["camera_id"])


def distort(x, y, coefficients):
    """OpenCV Brown-Conrady model on normalized coordinates; scalars or arrays."""
    k1, k2, p1, p2, k3 = coefficients
    r2 = x * x + y * y
    radial = 1 + r2 * (k1 + r2 * (k2 + r2 * k3))
    return (x * radial + 2 * p1 * x * y + p2 * (r2 + 2 * x * x),
            y * radial + p1 * (r2 + 2 * y * y) + 2 * p2 * x * y)


def project(camera, point):
    """Pixel of a camera-frame point, or None behind the camera."""
    if point[2] <= 0.001:
        return None
    x, y = distort(point[0] / point[2], point[1] / point[2], camera["distortion"])
    k = camera["intrinsics"]
    return k[0][0] * x + k[0][1] * y + k[0][2], k[1][1] * y + k[1][2]


def undistortion_map(camera):
    """Ideal normalized coordinates seen through every output pixel center."""
    width, height = camera["image_size_px"]
    k = camera["intrinsics"]
    v, u = np.mgrid[0:height, 0:width].astype(np.float64)
    yd = (v - k[1][2]) / k[1][1]
    xd = (u - k[0][2] - k[0][1] * yd) / k[0][0]
    k1, k2, p1, p2, k3 = camera["distortion"]
    x, y = xd.copy(), yd.copy()
    # Same fixed-point inversion as OpenCV undistortPoints.
    for _ in range(64):
        r2 = x * x + y * y
        radial = 1 + r2 * (k1 + r2 * (k2 + r2 * k3))
        x = (xd - 2 * p1 * x * y - p2 * (r2 + 2 * x * x)) / radial
        y = (yd - p1 * (r2 + 2 * y * y) - 2 * p2 * x * y) / radial
    ex, ey = distort(x, y, camera["distortion"])
    residual_px = np.hypot(ex - xd, ey - yd) * k[0][0]
    require(np.isfinite(residual_px).all() and residual_px.max() < 1e-3,
            f"camera {camera['id']}: lens distortion is not invertible across the image")
    return x, y


def overscan_camera(camera, x, y):
    """Skew-free pinhole that sees every ideal ray the distorted output needs."""
    fx, fy = camera["intrinsics"][0][0], camera["intrinsics"][1][1]
    margin = 2
    width = math.ceil((x.max() - x.min()) * fx) + 2 * margin + 1
    height = math.ceil((y.max() - y.min()) * fy) + 2 * margin + 1
    out_width, out_height = camera["image_size_px"]
    require(width <= MAX_OVERSCAN * out_width and height <= MAX_OVERSCAN * out_height,
            f"camera {camera['id']}: lens field of view needs more than {MAX_OVERSCAN}x render overscan")
    return {"intrinsics": [[fx, 0, margin - x.min() * fx], [0, fy, margin - y.min() * fy], [0, 0, 1]],
            "image_size_px": [width, height]}


def sample(source, xs, ys):
    """Bilinear lookup at OpenCV pixel-center coordinates; outside the render is black."""
    height, width, _ = source.shape
    x0, y0 = np.floor(xs).astype(int), np.floor(ys).astype(int)
    wx, wy = (xs - x0)[..., None], (ys - y0)[..., None]
    result = np.zeros(xs.shape + (source.shape[2],))
    for dy, weight_y in ((0, 1 - wy), (1, wy)):
        for dx, weight_x in ((0, 1 - wx), (1, wx)):
            xi, yi = x0 + dx, y0 + dy
            inside = ((xi >= 0) & (xi < width) & (yi >= 0) & (yi < height))[..., None]
            value = source[np.clip(yi, 0, height - 1), np.clip(xi, 0, width - 1)]
            result += np.where(inside, value, 0) * weight_x * weight_y
    return result


def pose_at(track, time_us):
    """Pose between authored frames: linear position, slerped rotation."""
    for before, after in zip(track, track[1:]):
        span = after["elapsed_us"] - before["elapsed_us"]
        if span > 0 and before["elapsed_us"] <= time_us <= after["elapsed_us"]:
            t = (time_us - before["elapsed_us"]) / span
            a, b = before["camera_to_world"], after["camera_to_world"]
            return (Vector(a["position_m"]).lerp(Vector(b["position_m"]), t),
                    quaternion(a["orientation_xyzw"]).slerp(quaternion(b["orientation_xyzw"]), t))
    raise ValueError("exposure extends past the camera's last authored pose")


def read_scene(path, validator):
    def strict_object(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, f"duplicate JSON key: {key}")
            result[key] = value
        return result

    def invalid_constant(value):
        raise ValueError(f"nonfinite JSON number: {value}")

    if Path(path).suffix == ".json":
        with Path(path).open("rb") as stream:
            payload = stream.read(64 * 1024 * 1024 + 1)
        require(len(payload) <= 64 * 1024 * 1024, "scene JSON exceeds 64 MiB")
        document = json.loads(payload, object_pairs_hook=strict_object, parse_constant=invalid_constant)
        require("samples" not in document, "traces must use CBOR")
        return document
    subprocess.run([validator, "validate-trace", str(path)], check=True, stdout=subprocess.DEVNULL)
    dumped = subprocess.run([validator, "dump", str(path)], check=True, capture_output=True)
    document = json.loads(dumped.stdout)
    require(document.get("scene") is not None, "trace has no scene")
    return document["scene"]


def material(name, color=None, checker=False):
    result = bpy.data.materials.new(name)
    result.use_nodes = True
    nodes = result.node_tree.nodes
    nodes.clear()
    output = nodes.new("ShaderNodeOutputMaterial")
    emission = nodes.new("ShaderNodeEmission")
    emission.inputs["Strength"].default_value = 1
    result.node_tree.links.new(emission.outputs[0], output.inputs["Surface"])
    if checker:
        texture = nodes.new("ShaderNodeTexImage")
        image = bpy.data.images.new("Projector checker", width=24, height=24, alpha=False)
        image.colorspace_settings.name = "Non-Color"
        pixels = []
        for y in range(24):
            for x in range(24):
                if x < 2 and y >= 22:
                    color = (0, 0.05, 0.8)
                elif x < 2 and y < 2:
                    color = (0, 0.8, 0.05)
                else:
                    color = (0.08,) * 3 if (x // 2 + y // 2) % 2 == 0 else (0.32,) * 3
                pixels.extend((*color, 1))
        image.pixels = pixels
        image.pack()
        texture.image = image
        texture.interpolation = "Closest"
        coordinates = nodes.new("ShaderNodeTexCoord")
        result.node_tree.links.new(coordinates.outputs["UV"], texture.inputs["Vector"])
        result.node_tree.links.new(texture.outputs["Color"], emission.inputs["Color"])
    else:
        emission.inputs["Color"].default_value = (*color, 1)
    return result


def build_scene(scene):
    check_render_support(scene)
    bpy.ops.wm.read_factory_settings(use_empty=True)
    world = bpy.data.worlds.new("Odyssey dark background")
    world.use_nodes = True
    world.node_tree.nodes["Background"].inputs["Color"].default_value = (0, 0, 0, 1)
    bpy.context.scene.world = world
    bpy.data.texts.new(METADATA).write(json.dumps(scene, allow_nan=False))
    screen_material = material("Projector UV checker", checker=True)
    marker_material = material("Canonical calibration markers", (1, 0, 0))
    for screen in scene["screens"]:
        columns, rows, positions, triangles = surface_geometry(screen)
        mesh = bpy.data.meshes.new(f"Screen {screen['id']} canonical grid")
        mesh.from_pydata(positions, [], triangles)
        mesh.update()
        obj = bpy.data.objects.new(f"Screen {screen['id']}", mesh)
        bpy.context.collection.objects.link(obj)
        obj["odyssey_kind"] = "screen"
        obj["screen_id"] = screen["id"]
        obj["columns"], obj["rows"] = columns, rows
        obj.rotation_mode = "QUATERNION"
        obj.rotation_quaternion = quaternion(screen["calibration"]["rotation"]).inverted()
        uv = mesh.uv_layers.new(name="ProjectorUV")
        for polygon in mesh.polygons:
            for loop_index in polygon.loop_indices:
                vertex = mesh.loops[loop_index].vertex_index
                uv.data[loop_index].uv = (vertex % columns / (columns - 1), 1 - vertex // columns / (rows - 1))
        mesh.materials.append(screen_material)
        inverse_rotation = quaternion(screen["calibration"]["rotation"]).inverted()
        points = screen["calibration"]["object_points"]
        require(len(points) == len(screen["marker_ids"]) == 6, "requires six canonical markers")
        for marker_id, point in zip(screen["marker_ids"], points):
            # Sphere center is the canonical point. No surface snapping, no fabricated tracker label.
            bpy.ops.mesh.primitive_uv_sphere_add(segments=16, ring_count=8, radius=0.0075,
                                               location=inverse_rotation @ Vector(point))
            marker = bpy.context.object
            marker.name = f"Marker {marker_id}"
            marker["odyssey_kind"] = "marker"
            marker["marker_id"] = marker_id
            marker["screen_id"] = screen["id"]
            marker.data.materials.append(marker_material)
            marker["odyssey_marker_mesh"] = mesh_digest(marker.data)
    for camera in scene["cameras"]:
        data = bpy.data.cameras.new(camera["id"])
        obj = bpy.data.objects.new(f"Camera {camera['id']}", data)
        bpy.context.collection.objects.link(obj)
        obj["odyssey_kind"], obj["camera_id"] = "camera", camera["id"]
    configure_render()
    apply_frame(scene, scene["frames"][0])
    for camera in scene["cameras"]:
        obj = tagged("camera", "camera_id", camera["id"])
        obj["odyssey_imported_camera"] = json.dumps(camera_state(obj))


def camera_state(obj):
    data = obj.data
    return {"matrix_world": [list(row) for row in obj.matrix_world],
            "type": data.type, "sensor_fit": data.sensor_fit,
            "sensor_width": data.sensor_width, "sensor_height": data.sensor_height,
            "lens": data.lens, "shift_x": data.shift_x, "shift_y": data.shift_y,
            "clip_start": data.clip_start, "clip_end": data.clip_end}


def check_camera_edits(scene):
    for camera in scene["cameras"]:
        obj = tagged("camera", "camera_id", camera["id"])
        require(not obj.constraints and not obj.animation_data and not obj.data.animation_data,
                "camera animation/constraints unsupported; author poses in scene JSON")
        require(json.loads(obj["odyssey_imported_camera"]) == camera_state(obj),
                f"camera {camera['id']} edited; author calibrated intrinsics/poses in scene JSON instead")


def configure_render():
    scene = bpy.context.scene
    scene.render.engine = "CYCLES"
    scene.cycles.device = "CPU"
    scene.cycles.samples = 8
    scene.cycles.use_denoising = False
    scene.render.image_settings.file_format = "PNG"
    scene.render.image_settings.color_mode = "RGB"
    scene.render.resolution_percentage = 100
    scene.render.film_transparent = False
    scene.render.use_motion_blur = False
    scene.view_settings.view_transform = "Standard"
    scene.view_settings.look = "None"
    scene.view_settings.exposure = 0
    scene.view_settings.gamma = 1


def tagged(kind, key, identifier):
    matches = [obj for obj in bpy.context.scene.objects
               if obj.get("odyssey_kind") == kind and obj.get(key) == identifier]
    require(len(matches) == 1, f"missing/duplicate {kind} {identifier}")
    return matches[0]


def apply_camera(camera, intrinsics, size, position, rotation):
    """Point the tagged Blender camera like a skew-free OpenCV pinhole."""
    obj = tagged("camera", "camera_id", camera["id"])
    width, height = size
    fx, fy = intrinsics[0][0], intrinsics[1][1]
    cx, cy = intrinsics[0][2], intrinsics[1][2]
    render = bpy.context.scene.render
    render.resolution_x, render.resolution_y = width, height
    render.pixel_aspect_x, render.pixel_aspect_y = 1, fx / fy
    obj.data.type = "PERSP"
    obj.data.sensor_fit = "HORIZONTAL"
    obj.data.sensor_width = 36
    obj.data.lens = fx * 36 / width
    # OpenCV puts pixel centers at integer coordinates, Blender raster centers sit at i+0.5.
    obj.data.shift_x = (width / 2 - cx - 0.5) / width
    obj.data.shift_y = (cy + 0.5 - height / 2) * (fx / fy) / width
    obj.data.clip_start, obj.data.clip_end = 0.001, 1000
    matrix = rotation.to_matrix().to_4x4()
    matrix.translation = position
    obj.matrix_world = matrix @ CV_TO_BLENDER
    bpy.context.scene.camera = obj
    bpy.context.view_layer.update()


def apply_frame(document, frame):
    camera = camera_of(document, frame)
    pose = frame["camera_to_world"]
    apply_camera(camera, camera["intrinsics"], camera["image_size_px"],
                 Vector(pose["position_m"]), quaternion(pose["orientation_xyzw"]))


def render_linear(path):
    settings = bpy.context.scene.render.image_settings
    settings.file_format, settings.color_mode, settings.color_depth = "OPEN_EXR", "RGB", "32"
    bpy.context.scene.render.filepath = str(path)
    bpy.ops.render.render(write_still=True)
    image = bpy.data.images.load(str(path))
    width, height = image.size
    pixels = np.empty(width * height * 4, np.float32)
    image.pixels.foreach_get(pixels)
    bpy.data.images.remove(image)
    return pixels.reshape(height, width, 4)[::-1, :, :3].astype(np.float64)


def render_frame(scene, frame, scratch):
    """Linear RGB image as the camera records it, top row first."""
    camera = camera_of(scene, frame)
    remap = any(camera["distortion"]) or camera["intrinsics"][0][1] != 0
    if remap:
        x, y = undistortion_map(camera)
        source = overscan_camera(camera, x, y)
    else:
        source = camera
    if frame["exposure_us"]:
        track = camera_track(scene, frame["camera_id"])
        poses = [pose_at(track, frame["elapsed_us"] + (i + 0.5) / EXPOSURE_SAMPLES * frame["exposure_us"])
                 for i in range(EXPOSURE_SAMPLES)]
    else:
        pose = frame["camera_to_world"]
        poses = [(Vector(pose["position_m"]), quaternion(pose["orientation_xyzw"]))]
    image = 0
    for index, (position, rotation) in enumerate(poses):
        apply_camera(camera, source["intrinsics"], source["image_size_px"], position, rotation)
        image = image + render_linear(scratch / f"sample-{index}.exr")
    image = image / len(poses)
    if not remap:
        return image
    k = source["intrinsics"]
    return sample(image, k[0][0] * x + k[0][2], k[1][1] * y + k[1][2])


def gaussian_blur(image, sigma):
    radius = math.ceil(3 * sigma)
    offsets = np.arange(-radius, radius + 1)
    kernel = np.exp(-0.5 * (offsets / sigma) ** 2)
    kernel /= kernel.sum()
    for axis in (0, 1):
        padding = [(radius, radius) if index == axis else (0, 0) for index in range(image.ndim)]
        padded = np.pad(image, padding, mode="edge")
        image = sum(weight * np.take(padded, range(offset + radius, offset + radius + image.shape[axis]), axis=axis)
                    for offset, weight in zip(offsets, kernel))
    return image


def apply_sensor(rgb, sensor, frame_index):
    """Raw sensor counts in [0, 1]: point spread, shot and read noise, full-well clip, ADC.

    Linear intensity 1.0 fills the well. Noise is seeded per frame so renders repeat.
    """
    if sensor["blur_sigma_px"] > 0:
        rgb = gaussian_blur(rgb, sensor["blur_sigma_px"])
    generator = np.random.default_rng([sensor["seed"], frame_index])
    full_well = sensor["full_well_electrons"]
    # Far past saturation the count no longer matters, and the cap keeps Poisson in range.
    electrons = generator.poisson(np.clip(rgb, 0, 1e3) * full_well).astype(np.float64)
    electrons += generator.normal(0, sensor["read_noise_electrons"], electrons.shape)
    levels = 2 ** sensor["bit_depth"] - 1
    return np.round(np.clip(electrons / full_well, 0, 1) * levels) / levels


def write_png(rgb, path, linear_bits=None):
    """sRGB display PNG, or linear counts when `linear_bits` gives the ADC depth."""
    height, width, _ = rgb.shape
    image = bpy.data.images.new("odyssey-frame", width, height, float_buffer=True, alpha=False)
    rgba = np.ones((height, width, 4), np.float32)
    rgba[..., :3] = rgb
    image.pixels.foreach_set(rgba[::-1].ravel())
    scene = bpy.context.scene
    settings = scene.render.image_settings
    depth = "16" if linear_bits and linear_bits > 8 else "8"
    settings.file_format, settings.color_mode, settings.color_depth = "PNG", "RGB", depth
    view = scene.view_settings.view_transform
    if linear_bits:
        scene.view_settings.view_transform = "Raw"
    image.save_render(str(path), scene=scene)
    scene.view_settings.view_transform = view
    bpy.data.images.remove(image)


def load_counts(path):
    """Stored PNG values without any color decoding, top row first."""
    image = bpy.data.images.load(str(path))
    image.colorspace_settings.name = "Non-Color"
    image.reload()
    width, height = image.size
    pixels = np.empty(width * height * 4, np.float32)
    image.pixels.foreach_get(pixels)
    bpy.data.images.remove(image)
    return pixels.reshape(height, width, 4)[::-1, :, :3].astype(np.float64)


def load_rgb(path):
    """Linear RGB decoded from an sRGB PNG, top row first.

    Measure in linear light: on encoded values a blurred marker over a varying
    background is weighted unevenly and its centroid shifts.
    """
    image = bpy.data.images.load(str(path))
    width, height = image.size
    pixels = np.empty(width * height * 4, np.float32)
    image.pixels.foreach_get(pixels)
    bpy.data.images.remove(image)
    encoded = pixels.reshape(height, width, 4)[::-1, :, :3].astype(np.float64)
    return np.where(encoded <= 0.04045, encoded / 12.92, ((encoded + 0.055) / 1.055) ** 2.4)


def mesh_digest(mesh):
    geometry = [[list(vertex.co) for vertex in mesh.vertices],
                [list(polygon.vertices) for polygon in mesh.polygons]]
    return hashlib.sha256(json.dumps(geometry).encode()).hexdigest()


def exported_scene(validator):
    require(METADATA in bpy.data.texts, "blend missing embedded scene metadata")
    scene = json.loads(bpy.data.texts[METADATA].as_string())
    check_render_support(scene)
    original_screens = json.dumps(scene["screens"], sort_keys=True)
    check_camera_edits(scene)
    check_static_asset()
    identities = {("screen", screen["id"]) for screen in scene["screens"]}
    identities |= {("marker", identifier) for screen in scene["screens"] for identifier in screen["marker_ids"]}
    identities |= {("camera", camera["id"]) for camera in scene["cameras"]}
    for obj in bpy.context.scene.objects:
        kind = obj.get("odyssey_kind")
        if kind is not None:
            key = {"screen": "screen_id", "marker": "marker_id", "camera": "camera_id"}.get(kind)
            require(key is not None and (kind, obj.get(key)) in identities, "tagged scene identity changed")
    for screen in scene["screens"]:
        obj = tagged("screen", "screen_id", screen["id"])
        require(obj.type == "MESH" and not obj.modifiers, "apply mesh modifiers before exporting")
        columns, rows, original, triangles = surface_geometry(screen)
        mesh = obj.data
        require(obj["columns"] == columns and obj["rows"] == rows, "grid dimensions changed")
        require(len(mesh.vertices) == columns * rows and
                [tuple(poly.vertices) for poly in mesh.polygons] == triangles,
                "screen topology changed; keep canonical row-major triangles")
        require(mesh.uv_layers.active is not None, "screen UV layer missing")
        for loop in mesh.loops:
            vertex = loop.vertex_index
            expected = Vector((vertex % columns / (columns - 1), 1 - vertex // columns / (rows - 1)))
            require((mesh.uv_layers.active.data[loop.index].uv - expected).length < 1e-6,
                    "screen UVs changed; schema requires uniform projector UV")
        rotation = quaternion(screen["calibration"]["rotation"])
        positions = [list(rotation @ (obj.matrix_world @ vertex.co)) for vertex in mesh.vertices]
        changed = any((Vector(before) - Vector(after)).length > 1e-6
                      for before, after in zip(original, positions))
        if changed:
            screen["calibration"]["surface"] = {"columns": columns, "rows": rows, "positions_m": positions}
        for index, marker_id in enumerate(screen["marker_ids"]):
            marker = tagged("marker", "marker_id", marker_id)
            require(marker.get("screen_id") == screen["id"], "marker moved to different screen ID")
            require(mesh_digest(marker.data) == marker["odyssey_marker_mesh"]
                    and all(abs(value - 1) < 1e-6 for value in marker.matrix_world.to_scale()),
                    "canonical marker shape/size edited; only marker position may change")
            physical = list(rotation @ marker.matrix_world.translation)
            old = screen["calibration"]["object_points"][index]
            if (Vector(old) - Vector(physical)).length > 1e-6:
                screen["calibration"]["object_points"][index] = physical
    if json.dumps(scene["screens"], sort_keys=True) != original_screens or environment_meshes():
        with tempfile.TemporaryDirectory(prefix="odyssey-scene-labels-") as directory:
            source, target = Path(directory) / "input.json", Path(directory) / "output.json"
            source.write_text(json.dumps(scene, allow_nan=False))
            command = [validator, "relabel-scene", str(source), str(target)]
            if environment_meshes():
                asset = Path(directory) / "environment.glb"
                export_asset(scene, asset, validator)
                command.extend(["--asset", str(asset)])
            subprocess.run(command, check=True)
            scene = json.loads(target.read_text())
        update_marker_visibility(scene)
    return scene


def environment_meshes():
    return [obj for obj in bpy.context.scene.objects
            if obj.type == "MESH" and obj.get("odyssey_kind") not in ("screen", "marker")]


def check_static_asset():
    for obj in bpy.context.scene.objects:
        require(obj.type in ("MESH", "EMPTY", "CAMERA"), f"unsupported glTF object type: {obj.type}")
        require(not obj.animation_data and not obj.constraints, "animated/constrained objects unsupported")
        if obj.type == "MESH":
            require(not obj.modifiers and not obj.data.shape_keys and not obj.data.animation_data,
                    "apply static modifiers; skins/morphs/animation unsupported")
            for slot in obj.material_slots:
                surface_material = slot.material
                if surface_material and surface_material.use_nodes:
                    require(not surface_material.animation_data and not surface_material.node_tree.animation_data,
                            "animated materials unsupported")
                    for node in surface_material.node_tree.nodes:
                        require(node.type not in ("BSDF_TRANSPARENT", "HOLDOUT", "VOLUME_PRINCIPLED",
                                                  "VOLUME_ABSORPTION", "VOLUME_SCATTER"),
                                "transparent/volume materials unsupported")
                        if node.type == "BSDF_PRINCIPLED":
                            alpha = node.inputs["Alpha"]
                            transmission = node.inputs["Transmission Weight"]
                            require(not alpha.is_linked and alpha.default_value == 1
                                    and not transmission.is_linked and transmission.default_value == 0,
                                    "alpha/transmission materials unsupported")
        if obj.type == "CAMERA":
            require(obj.get("odyssey_kind") == "camera", "extra cameras unsupported; author camera in scene JSON")


def export_asset(scene, path, validator):
    path = Path(path)
    require(not path.exists(), f"refusing to overwrite asset: {path}")
    check_static_asset()
    roots = [obj for obj in bpy.context.scene.objects if "odyssey_scene_json" in obj]
    require(len(roots) <= 1, "duplicate glTF calibration metadata")
    if roots:
        root = roots[0]
    else:
        root = bpy.data.objects.new("Odyssey calibration sidecar", None)
        bpy.context.collection.objects.link(root)
    root["odyssey_scene_json"] = json.dumps(scene, allow_nan=False)
    bpy.ops.export_scene.gltf(filepath=str(path.resolve()), export_format="GLB",
                             export_extras=True, export_yup=True, export_cameras=False,
                             export_animations=False, export_skins=False, export_morph=False)
    subprocess.run([validator, "validate-asset", str(path)], check=True)
    return {"path": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "asset_to_world": [[1, 0, 0, 0], [0, 0, -1, 0], [0, 1, 0, 0], [0, 0, 0, 1]]}


def update_marker_visibility(scene):
    bpy.context.view_layer.update()
    graph = bpy.context.evaluated_depsgraph_get()
    for frame in scene["frames"]:
        camera = camera_of(scene, frame)
        origin = Vector(frame["camera_to_world"]["position_m"])
        inverse = quaternion(frame["camera_to_world"]["orientation_xyzw"]).inverted()
        width, height = camera["image_size_px"]
        visible = []
        for screen in scene["screens"]:
            for marker_id in screen["marker_ids"]:
                marker = tagged("marker", "marker_id", marker_id)
                delta = marker.matrix_world.translation - origin
                pixel = project(camera, inverse @ delta)
                if pixel is None or not (-0.5 <= pixel[0] < width - 0.5 and -0.5 <= pixel[1] < height - 0.5):
                    continue
                hit, _, _, _, obj, _ = bpy.context.scene.ray_cast(
                    graph, origin, delta.normalized(), distance=delta.length + 0.01)
                if hit and obj.original == marker.original:
                    visible.append(marker_id)
        frame["visible_marker_ids"] = visible


def write_json(path, document):
    with Path(path).open("x") as stream:
        json.dump(document, stream, indent=2, allow_nan=False)
        stream.write("\n")


def render_frames(scene, output, requested, validator):
    check_render_support(scene)
    frames = scene["frames"]
    if requested:
        require(len(set(requested)) == len(requested), "duplicate requested frame")
        require(set(requested) <= {frame["id"] for frame in frames}, "unknown requested frame ID")
        frames = [frame for frame in frames if frame["id"] in requested]
    output = Path(output)
    output.mkdir(parents=True, exist_ok=False)
    manifest = {"schema_version": 1, "scene_schema_version": 1,
                "source_blend": bpy.data.filepath, "scene": scene, "images": []}
    manifest["visual_asset"] = export_asset(scene, output / "scene.glb", validator)
    write_json(output / "scene.json", scene)
    configure_render()
    with tempfile.TemporaryDirectory(prefix="odyssey-render-") as scratch:
        for index, frame in enumerate(frames):
            # IDs never become paths, so an arbitrary scene ID cannot escape the output directory.
            filename = f"frame-{index:06d}.png"
            rgb = render_frame(scene, frame, Path(scratch))
            camera = camera_of(scene, frame)
            sensor = camera.get("sensor")
            if sensor:
                rgb = apply_sensor(rgb, sensor, scene["frames"].index(frame))
            write_png(rgb, output / filename, sensor["bit_depth"] if sensor else None)
            manifest["images"].append({"path": filename, "frame_id": frame["id"], "camera_id": camera["id"],
                                       "source_id": camera["source_id"], "elapsed_us": frame["elapsed_us"],
                                       "exposure_us": frame["exposure_us"], "image_size_px": camera["image_size_px"],
                                       "encoding": "linear_sensor_counts" if sensor else "srgb"})
    apply_frame(scene, scene["frames"][0])
    write_json(output / "manifest.json", manifest)
    return manifest


def selfcheck(output, validator):
    output = Path(output)
    output.mkdir(parents=True, exist_ok=False)
    points = [[x, y, 0] for x, y in ((0.12, 0.1), (0.12, 0.36), (0.56, 0.36), (0.64, 0.1), (0.96, 0.1), (0.96, 0.36))]
    scene = {"schema_version": 1, "provenance": "Blender independent pinhole/roundtrip selfcheck",
             "coordinate_basis": "screen_x_right_y_down_z_forward",
             "screens": [{"id": 0, "calibration": {"rotation": [0, 0, 0, 1],
                          "homography": [1 / 1.2, 0, 0, 0, 1 / 0.5, 0, 0, 0, 1], "object_points": points},
                          "marker_ids": [f"marker-{i}" for i in range(6)]}],
             "cameras": [{"id": "camera", "source_id": "independent", "image_size_px": [400, 240],
                          "intrinsics": [[480, 0, 187], [0, 450, 113], [0, 0, 1]], "distortion": [0] * 5}],
             "frames": [{"id": "off-center", "elapsed_us": 0, "exposure_us": 0, "camera_id": "camera",
                         "camera_to_world": {"position_m": [0.55, 0.24, -2], "orientation_xyzw": [0, 0, 0, 1]},
                         "visible_marker_ids": [f"marker-{i}" for i in range(6)],
                         "truth": {"aimpoint": [0.55 / 1.2, 0.24 / 0.5], "position_m": [0.55, -0.24, 2], "screen_id": 0}}]}
    validate(scene, validator)
    build_scene(scene)
    assert exported_scene(validator) == scene
    screen_mesh = tagged("screen", "screen_id", 0).data
    corner_uv = {loop.vertex_index: tuple(screen_mesh.uv_layers.active.data[loop.index].uv)
                 for loop in screen_mesh.loops if loop.vertex_index in (0, PLANAR_RESOLUTION * (PLANAR_RESOLUTION - 1))}
    assert corner_uv[0] == (0, 1)
    assert corner_uv[PLANAR_RESOLUTION * (PLANAR_RESOLUTION - 1)] == (0, 0)
    blend = output / "selfcheck.blend"
    bpy.ops.wm.save_as_mainfile(filepath=str(blend.resolve()), check_existing=False)
    manifest = render_frames(scene, output / "images", None, validator)
    image = bpy.data.images.load(str((output / "images" / manifest["images"][0]["path"]).resolve()))
    pixels = list(image.pixels)
    width, height = image.size
    for logical_v, channel in ((0.04, 2), (0.96, 1)):
        column = round(480 * (0.04 * 1.2 - 0.55) / 2 + 187)
        row = round(450 * (logical_v * 0.5 - 0.24) / 2 + 113)
        offset = ((height - 1 - row) * width + column) * 4
        color = pixels[offset:offset + 3]
        assert color[channel] > 0.5 and color[0] < 0.1, ("UV anchor mirrored", logical_v, color)
    for x, y, z in points:
        u = 480 * (x - 0.55) / (z + 2) + 187
        v = 450 * (y - 0.24) / (z + 2) + 113
        hits = []
        for row in range(max(0, math.floor(v) - 4), min(height, math.ceil(v) + 5)):
            for column in range(max(0, math.floor(u) - 4), min(width, math.ceil(u) + 5)):
                offset = ((height - 1 - row) * width + column) * 4
                red, green, blue = pixels[offset:offset + 3]
                if red > 0.5 and green < 0.1 and blue < 0.1:
                    hits.append((column, row))
        assert len(hits) >= 3, ("missing rendered canonical marker", u, v, hits)
        centroid = tuple(sum(p[axis] for p in hits) / len(hits) for axis in range(2))
        assert math.hypot(centroid[0] - u, centroid[1] - v) < 0.8, (centroid, (u, v))
    mesh = tagged("screen", "screen_id", 0)
    center = (PLANAR_RESOLUTION // 2 - 1) * PLANAR_RESOLUTION + PLANAR_RESOLUTION // 2 - 3
    mesh.data.vertices[center].co.z = 0.025
    mesh.data.vertices[center].co.x += 0.004
    marker = tagged("marker", "marker_id", "marker-0")
    marker.location.x += 0.01
    bpy.context.view_layer.update()
    edited = exported_scene(validator)
    surface = edited["screens"][0]["calibration"]["surface"]
    assert abs(surface["positions_m"][center][2] - 0.025) < 1e-6
    assert abs(edited["screens"][0]["calibration"]["object_points"][0][0] - 0.13) < 1e-6
    # Relabel round-trips through Rust's f32 ScreenCalibration, so compare at f32 precision.
    assert all(math.isclose(a, b, rel_tol=1e-6, abs_tol=1e-7) for a, b in zip(
        edited["screens"][0]["calibration"]["homography"],
        scene["screens"][0]["calibration"]["homography"]))
    assert edited["screens"][0]["marker_ids"] == scene["screens"][0]["marker_ids"]
    assert edited["frames"][0]["camera_to_world"] == scene["frames"][0]["camera_to_world"]
    assert abs(edited["frames"][0]["truth"]["aimpoint"][0] - scene["frames"][0]["truth"]["aimpoint"][0]) > 1e-5
    validate(edited, validator)
    write_json(output / "edited-scene.json", edited)
    build_scene(edited)
    reimported = exported_scene(validator)
    for before, after in zip(surface["positions_m"], reimported["screens"][0]["calibration"]["surface"]["positions_m"]):
        assert (Vector(before) - Vector(after)).length < 1e-6
    camera = tagged("camera", "camera_id", "camera")
    camera.data.shift_x += 0.01
    try:
        exported_scene(validator)
    except ValueError as error:
        assert "edited" in str(error)
    else:
        raise AssertionError("edited camera silently accepted")
    build_scene(scene)
    bpy.ops.mesh.primitive_cube_add(size=1, location=(0.335, 0.17, -1))
    occluder = bpy.context.object
    occluder.name = "Authored static occluder"
    occluder.dimensions = (0.045, 0.045, 0.03)
    occluder.data.materials.append(material("Opaque occluder", (0.02, 0.02, 0.02)))
    bpy.ops.mesh.primitive_uv_sphere_add(segments=16, ring_count=8, radius=0.015,
                                       location=(0.8, 0.24, -0.2))
    distractor = bpy.context.object
    distractor.name = "Authored bright non-marker distractor"
    distractor.data.materials.append(material("Bright distractor", (1, 1, 1)))
    bpy.context.view_layer.update()
    occluded = exported_scene(validator)
    assert "marker-0" not in occluded["frames"][0]["visible_marker_ids"]
    assert set(occluded["frames"][0]["visible_marker_ids"]) == set(scene["frames"][0]["visible_marker_ids"]) - {"marker-0"}
    environment_blend = output / "authored-environment.blend"
    bpy.ops.wm.save_as_mainfile(filepath=str(environment_blend.resolve()), check_existing=False)
    rendered = render_frames(occluded, output / "authored-environment", None, validator)
    assert rendered["visual_asset"]["sha256"] == hashlib.sha256((output / "authored-environment" / "scene.glb").read_bytes()).hexdigest()
    check_optics(scene, points, output, validator)
    check_sensor(scene, points, output, validator)
    print("Blender selfcheck passed: image centroids, canonical UVs, edited geometry, marker roundtrip, "
          "lens distortion, exposure integration and sensor model")


def red_centroid(rgb, center, half_width, half_height):
    """Centroid and horizontal spread of marker-red light in a window."""
    cu, cv = round(center[0]), round(center[1])
    window = rgb[cv - half_height:cv + half_height + 1, cu - half_width:cu + half_width + 1]
    weight = np.clip(window[..., 0] - np.maximum(window[..., 1], window[..., 2]), 0, None)
    total = weight.sum()
    require(total > 0, f"no marker light near {center}")
    rows, columns = np.mgrid[cv - half_height:cv + half_height + 1, cu - half_width:cu + half_width + 1]
    u, v = (weight * columns).sum() / total, (weight * rows).sum() / total
    spread = math.sqrt((weight * (columns - u) ** 2).sum() / total)
    return u, v, spread


def check_optics(scene, points, output, validator):
    distorted = copy.deepcopy(scene)
    camera = distorted["cameras"][0]
    camera["distortion"] = [-0.6, 0.25, 0.0015, -0.001, 0.0]
    camera["intrinsics"][0][1] = 1.5
    validate(distorted, validator)
    build_scene(distorted)
    manifest = render_frames(distorted, output / "distorted", None, validator)
    rgb = load_rgb(output / "distorted" / manifest["images"][0]["path"])
    origin = Vector(distorted["frames"][0]["camera_to_world"]["position_m"])
    largest_shift = 0.0
    for point in points:
        expected = project(camera, Vector(point) - origin)
        ideal = project({**camera, "distortion": [0] * 5, "intrinsics": scene["cameras"][0]["intrinsics"]},
                        Vector(point) - origin)
        largest_shift = max(largest_shift, math.dist(expected, ideal))
        u, v, _ = red_centroid(rgb, expected, 5, 5)
        assert math.dist((u, v), expected) < 0.7, ("distorted marker", (u, v), expected)
    assert largest_shift > 3, ("distortion too weak to test", largest_shift)

    moving = copy.deepcopy(scene)
    start = moving["frames"][0]
    start["exposure_us"] = 20_000
    end = copy.deepcopy(start)
    end.update(id="moved", elapsed_us=20_000, exposure_us=0)
    end["camera_to_world"]["position_m"] = [0.63, 0.24, -2]
    end["truth"] = {"aimpoint": [0.63 / 1.2, 0.24 / 0.5], "position_m": [0.63, -0.24, 2], "screen_id": 0}
    moving["frames"] = [start, end]
    validate(moving, validator)
    build_scene(moving)
    manifest = render_frames(moving, output / "exposure", [start["id"]], validator)
    rgb = load_rgb(output / "exposure" / manifest["images"][0]["path"])
    camera = moving["cameras"][0]
    streak_px = 0.08 * 480 / 2
    for point in points:
        middle = project(camera, Vector(point) - Vector((0.59, 0.24, -2)))
        u, v, spread = red_centroid(rgb, middle, 16, 5)
        assert math.dist((u, v), middle) < 0.7, ("streak centroid", (u, v), middle)
        # A uniform streak of length L has standard deviation L / sqrt(12).
        assert spread > 0.8 * streak_px / math.sqrt(12), ("streak too short", spread)


def check_sensor(scene, points, output, validator):
    sensor = {"blur_sigma_px": 1.2, "full_well_electrons": 2000.0, "read_noise_electrons": 6.0,
              "bit_depth": 10, "seed": 3}
    noisy = copy.deepcopy(scene)
    noisy["cameras"][0]["sensor"] = sensor
    validate(noisy, validator)
    build_scene(noisy)
    manifest = render_frames(noisy, output / "sensor", None, validator)
    assert manifest["images"][0]["encoding"] == "linear_sensor_counts"
    counts = load_counts(output / "sensor" / manifest["images"][0]["path"])
    configure_render()
    with tempfile.TemporaryDirectory(prefix="odyssey-sensor-check-") as scratch:
        ideal = render_frame(noisy, noisy["frames"][0], Path(scratch))
    expected = apply_sensor(ideal, sensor, 0)
    levels = 2 ** sensor["bit_depth"] - 1
    assert np.abs(counts - expected).max() <= 0.5 / levels + 1 / 65535, "PNG differs from the sensor model"

    # Shot plus read plus quantization noise, normalized, should have unit spread.
    blurred = gaussian_blur(ideal, sensor["blur_sigma_px"])
    well = sensor["full_well_electrons"]
    variance = (blurred * well + sensor["read_noise_electrons"] ** 2) / well ** 2 + 1 / (12 * levels ** 2)
    unclipped = (blurred > 0.02) & (blurred < 0.9)
    normalized = (counts - blurred)[unclipped] / np.sqrt(variance[unclipped])
    assert unclipped.sum() > 10_000 and abs(normalized.std() - 1) < 0.05, normalized.std()
    assert abs(normalized.mean()) < 0.05, normalized.mean()

    saturated = apply_sensor(np.full((4, 4, 3), 5.0), sensor, 0)
    assert (saturated == 1.0).all()

    origin = Vector(noisy["frames"][0]["camera_to_world"]["position_m"])
    _, _, sharp_spread = red_centroid(ideal, project(noisy["cameras"][0], Vector(points[2]) - origin), 6, 6)
    for point in points:
        expected_px = project(noisy["cameras"][0], Vector(point) - origin)
        u, v, spread = red_centroid(counts, expected_px, 6, 6)
        assert math.dist((u, v), expected_px) < 0.7, ("blurred noisy marker", (u, v), expected_px)
    assert spread > sharp_spread + 0.3, ("point spread not applied", spread, sharp_spread)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--validator", default="tracking-regression", help="Rust tracking-regression executable")
    commands = parser.add_subparsers(dest="command", required=True)
    build = commands.add_parser("build")
    build.add_argument("input")
    build.add_argument("output")
    render = commands.add_parser("render")
    render.add_argument("blend")
    render.add_argument("output")
    render.add_argument("--frame", action="append", help="scene frame ID; repeat to select frames")
    export = commands.add_parser("export")
    export.add_argument("blend")
    export.add_argument("output")
    check = commands.add_parser("selfcheck")
    check.add_argument("output")
    arguments = parser.parse_args(sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else [])
    if arguments.command == "selfcheck":
        selfcheck(arguments.output, arguments.validator)
    elif arguments.command == "build":
        require(not Path(arguments.output).exists(), "refusing to overwrite blend output")
        scene = read_scene(arguments.input, arguments.validator)
        validate(scene, arguments.validator)
        build_scene(scene)
        Path(arguments.output).parent.mkdir(parents=True, exist_ok=True)
        bpy.ops.wm.save_as_mainfile(filepath=str(Path(arguments.output).resolve()), check_existing=False)
    else:
        if arguments.command == "export":
            outputs = [Path(arguments.output), Path(arguments.output).with_suffix(".glb"),
                       Path(arguments.output).with_suffix(".manifest.json")]
            require(len(set(outputs)) == 3 and not any(path.exists() for path in outputs),
                    "refusing to overwrite scene/asset/manifest output")
        bpy.ops.wm.open_mainfile(filepath=str(Path(arguments.blend).resolve()), load_ui=False, use_scripts=False)
        scene = exported_scene(arguments.validator)
        validate(scene, arguments.validator)
        if arguments.command == "export":
            asset = export_asset(scene, Path(arguments.output).with_suffix(".glb"), arguments.validator)
            write_json(arguments.output, scene)
            write_json(Path(arguments.output).with_suffix(".manifest.json"),
                       {"schema_version": 1, "scene_schema_version": 1, "scene": scene,
                        "source_blend": bpy.data.filepath, "images": [], "visual_asset": asset})
        else:
            render_frames(scene, arguments.output, arguments.frame, arguments.validator)


if __name__ == "__main__":
    main()
