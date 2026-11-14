#!/usr/bin/env python3
"""Export calibrated Odyssey scenes/traces to real Rerun recordings, without a viewer."""
import argparse
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

import numpy as np

MAX_BYTES = 64 * 1024 * 1024
MAX_IMAGE_BYTES = 512 * 1024 * 1024
MAX_TOTAL_IMAGES = 2 * 1024 * 1024 * 1024
FLIP = np.diag([1.0, -1.0, -1.0])
TRUTH_COLOR = [60, 220, 100]
PREDICTED_COLOR = [255, 140, 40]


def require(condition, message):
    if not condition:
        raise ValueError(message)


def _unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, f"duplicate JSON key: {key}")
        result[key] = value
    return result


def load_json(path):
    with Path(path).open("rb") as stream:
        raw = stream.read(MAX_BYTES + 1)
    require(len(raw) <= MAX_BYTES, f"JSON exceeds {MAX_BYTES} bytes: {path}")
    value = json.loads(raw, object_pairs_hook=_unique_object,
                       parse_constant=lambda value: require(False, f"nonfinite JSON: {value}"))
    require(isinstance(value, dict), "JSON root must be an object")
    return value


def entity(identifier):
    return "id-" + str(identifier).encode("utf-8").hex()


def client_position(position):
    return FLIP @ np.asarray(position, dtype=float)


def client_rotation(rotation):
    return FLIP @ np.asarray(rotation, dtype=float) @ FLIP


def quaternion_matrix(quaternion):
    x, y, z, w = np.asarray(quaternion) / np.linalg.norm(quaternion)
    return np.array([[1 - 2*(y*y+z*z), 2*(x*y-z*w), 2*(x*z+y*w)],
                     [2*(x*y+z*w), 1 - 2*(x*x+z*z), 2*(y*z-x*w)],
                     [2*(x*z-y*w), 2*(y*z+x*w), 1 - 2*(x*x+y*y)]])


def physical_to_world(screen, points):
    return np.asarray(points) @ quaternion_matrix(screen["calibration"]["rotation"])


def uv_to_world(screen, uv):
    u, v = uv
    if not 0.0 <= u <= 1.0 or not 0.0 <= v <= 1.0:
        return None
    surface = screen["calibration"].get("surface")
    if surface is None:
        homography = np.asarray(screen["calibration"]["homography"]).reshape(3, 3, order="F")
        point = np.linalg.solve(homography, [u, v, 1.0])
        require(abs(point[2]) > 1e-12, "UV lies on calibration projective horizon")
        physical = [point[0] / point[2], point[1] / point[2], 0.0]
    else:
        columns, rows = surface["columns"], surface["rows"]
        x, y = u * (columns-1), v * (rows-1)
        column, row = min(int(x), columns-2), min(int(y), rows-2)
        s, t = x-column, y-row
        points = surface["positions_m"]
        tl_index = row*columns+column
        tl, tr, bl, br = (np.asarray(points[index]) for index in
                          (tl_index, tl_index+1, tl_index+columns, tl_index+columns+1))
        physical = ((1-s)*tl + (s-t)*tr + t*br if s >= t
                    else (1-t)*tl + s*br + (t-s)*bl)
    return physical_to_world(screen, physical)


def screen_mesh(screen):
    surface = screen["calibration"].get("surface")
    columns, rows = (surface["columns"], surface["rows"]) if surface else (17, 17)
    uv = [[x/(columns-1), y/(rows-1)] for y in range(rows) for x in range(columns)]
    positions = (physical_to_world(screen, surface["positions_m"]) if surface
                 else np.array([uv_to_world(screen, point) for point in uv]))
    triangles = []
    for row in range(rows-1):
        for column in range(columns-1):
            tl = row*columns+column
            triangles.extend([[tl, tl+1, tl+columns+1], [tl, tl+columns+1, tl+columns]])
    lines = [positions[row*columns:(row+1)*columns] for row in range(rows)]
    lines += [positions[column::columns] for column in range(columns)]
    return positions, np.asarray(triangles), np.asarray(uv), lines


def authoritative(validator, arguments, allow_failed_accuracy=False):
    # Temporary files bound process output without keeping an unbounded pipe in RAM.
    with tempfile.TemporaryDirectory(prefix="odyssey-validation-") as directory:
        stdout_path, stderr_path = Path(directory)/"stdout.json", Path(directory)/"stderr.txt"
        with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
            result = subprocess.run([validator, *map(str, arguments)], stdout=stdout,
                                    stderr=stderr, timeout=120, check=False)
        if result.returncode not in ((0, 1) if allow_failed_accuracy else (0,)):
            detail = stderr_path.read_bytes()[:4096].decode(errors="replace")
            raise ValueError(f"authoritative validator failed: {detail}")
        try:
            value = load_json(stdout_path)
        except (ValueError, OSError) as error:
            detail = stderr_path.read_bytes()[:4096].decode(errors="replace")
            raise ValueError(f"authoritative validator returned no valid JSON: {detail}") from error
        if allow_failed_accuracy:
            require("capture" in value and "metrics" in value and type(value.get("passed")) is bool,
                    "validator did not return an evaluation report")
            require(result.returncode == (0 if value["passed"] else 1), "validator exit/report mismatch")
        return value


def checked_asset(asset, root, validator):
    relative = Path(asset["path"])
    require(not relative.is_absolute(), "visual asset path must be relative")
    path = (root/relative).resolve()
    require(path.is_relative_to(root), "visual asset path escapes manifest directory")
    require(path.suffix.lower() == ".glb" and path.stat().st_size <= MAX_BYTES,
            "visual asset must be a bounded binary .glb")
    require(asset["asset_to_world"] == [[1, 0, 0, 0], [0, 0, -1, 0],
                                       [0, 1, 0, 0], [0, 0, 0, 1]],
            "visual asset transform must map glTF Y-up to Odyssey RDF")
    with path.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    require(digest == asset["sha256"], "visual asset sha256 mismatch")
    authoritative(validator, ["validate-asset", path])
    return {**asset, "path": path}


def load_input(path, validator):
    if Path(path).suffix == ".json":
        document = load_json(path)
        require("samples" not in document, "traces must be CBOR, not JSON")
        return document
    return authoritative(validator, ["dump", path])


def checked_inputs(input_path, capture_path, report_path, manifest_path, validator):
    document = load_input(input_path, validator)
    trace = document if "samples" in document else None
    scene = trace.get("scene") if trace else document
    if trace:
        authoritative(validator, ["validate-trace", input_path])
    require(trace is not None or scene is not None, "scene or trace required")
    with tempfile.TemporaryDirectory(prefix="odyssey-scene-") as directory:
        if scene:
            require(scene.get("schema_version") == 1, "scene schema_version must be 1")
            scene_path = Path(directory)/"scene.json"
            scene_path.write_text(json.dumps(scene, allow_nan=False))
            authoritative(validator, ["validate-scene", scene_path])
        report = None
        if capture_path or report_path:
            require(trace is not None, "predictions require an original trace, not standalone scene")
            if report_path:
                report = authoritative(validator, ["evaluate-report", input_path, report_path], True)
            else:
                report = authoritative(validator, ["evaluate", input_path, capture_path], True)
            outputs = report["capture"]["outputs"]
            require(len(outputs) == len(trace["samples"]) and
                    all(row["sample_index"] == index for index, row in enumerate(outputs)),
                    "capture must contain exact ordered sample indices")
    images = []
    visual_asset = None
    if manifest_path:
        manifest = load_json(manifest_path)
        require(scene is not None, "image manifest requires scene")
        require(manifest.get("schema_version") == 1 and manifest.get("scene_schema_version") == 1,
                "unsupported image manifest schema")
        require(manifest.get("scene") == scene, "image manifest scene differs from input scene")
        frames = {frame["id"]: frame for frame in scene["frames"]}
        cameras = {camera["id"]: camera for camera in scene["cameras"]}
        root = Path(manifest_path).resolve().parent
        if manifest.get("visual_asset") is not None:
            visual_asset = checked_asset(manifest["visual_asset"], root, validator)
        seen, total = set(), 0
        require(isinstance(manifest.get("images"), list), "manifest.images must be an array")
        require(len(manifest["images"]) <= len(frames), "too many manifest images")
        from PIL import Image
        for image in manifest["images"]:
            frame_id = image["frame_id"]
            require(frame_id in frames and frame_id not in seen, "unknown/duplicate image frame")
            seen.add(frame_id)
            frame = frames[frame_id]
            camera = cameras[frame["camera_id"]]
            require(all(image[key] == value for key, value in
                        [("camera_id", frame["camera_id"]), ("source_id", camera["source_id"]),
                         ("elapsed_us", frame["elapsed_us"]), ("exposure_us", frame["exposure_us"]),
                         ("image_size_px", camera["image_size_px"])]), "manifest frame identity mismatch")
            relative = Path(image["path"])
            require(not relative.is_absolute(), "manifest image paths must be relative")
            path = (root/relative).resolve()
            require(path.is_relative_to(root), "manifest image path escapes directory")
            size = path.stat().st_size
            total += size
            require(size <= MAX_IMAGE_BYTES and total <= MAX_TOTAL_IMAGES, "image byte limit exceeded")
            with Image.open(path) as decoded:
                require(list(decoded.size) == camera["image_size_px"], "image dimensions mismatch")
                decoded.verify()
            images.append((frame_id, path))
    return scene, trace, report, dict(images), visual_asset


def sdk():
    version = importlib.metadata.version("rerun-sdk")
    numbers = tuple(int(part) for part in version.split(".")[:2])
    require(numbers == (0, 38), f"unsupported rerun-sdk {version}; use declared shell (0.38.1)")
    import rerun as rr
    return rr, version


def set_clock(recording, name, index, elapsed_us):
    recording.reset_time()
    recording.set_time(name + "_index", sequence=index)
    recording.set_time(name + "_elapsed", duration=elapsed_us / 1_000_000)


def source_identity(trace, source):
    return {"source_id": source["id"], "device_uuid": source["device_uuid"],
            "clock_basis": source["clock_basis"], "trace_provenance": trace["provenance"],
            "provenance_kind": "declared, not inferred from pixels or device ID"}


def default_blueprint(scene, trace, screens):
    import rerun.blueprint as blueprint
    points = []
    for screen in screens.values():
        surface = screen["calibration"].get("surface")
        points.extend(physical_to_world(screen, surface["positions_m"]) if surface else
                      [uv_to_world(screen, uv) for uv in ([0, 0], [1, 0], [0, 1], [1, 1])])
    if scene:
        points.extend(frame["camera_to_world"]["position_m"] for frame in scene["frames"])
    bounds = np.asarray(points)
    center = (bounds.min(axis=0)+bounds.max(axis=0))/2
    span = max(float(np.ptp(bounds, axis=0).max()), 0.2)
    overview = blueprint.Spatial3DView(
        name="Calibrated world · green truth / orange estimate", origin="/world",
        eye_controls=blueprint.EyeControls3D(kind=blueprint.Eye3DKind.Orbital,
            position=center+span*np.array([0.75, -0.45, -0.9]), look_target=center, eye_up=[0, -1, 0]),
        line_grid=False)
    tabs = []
    if trace:
        tabs.append(blueprint.Spatial2DView(
            name="Aim UV · arrival clock", origin="/aim_uv",
            contents=["/aim_uv/truth/**", "/aim_uv/predicted/**"],
            visual_bounds=blueprint.VisualBounds2D(x_range=[-0.05, 1.05], y_range=[-0.05, 1.05])))
        source = entity(trace["sources"][0]["id"])
        tabs.append(blueprint.Spatial2DView(name="Raw NF pixels · arrival clock",
                    origin="/observed/"+source+"/nf"))
    if scene:
        camera = entity(scene["cameras"][0]["id"])
        tabs.append(blueprint.Spatial2DView(name="Reference image · select scene_index clock",
                    origin="/world/reference/"+camera+"/image"))
        tabs.append(blueprint.Spatial2DView(name="Reference UV · select scene_index clock",
                    origin="/aim_uv/reference",
                    visual_bounds=blueprint.VisualBounds2D(x_range=[-0.05, 1.05], y_range=[-0.05, 1.05])))
    tabs.append(blueprint.TextDocumentView(name="Provenance / calibration / evaluation", origin="/identity"))
    detail = blueprint.Tabs(*tabs)
    if trace:
        prefixes = ["/samples/"+entity(source["id"]) for source in trace["sources"]]
        detail = blueprint.Vertical(
            detail,
            blueprint.TimeSeriesView(name="Aim error · UV distance", contents=[p+"/error/aim_uv" for p in prefixes]),
            blueprint.TimeSeriesView(name="Position error · meters", contents=[p+"/error/position_m" for p in prefixes]),
            row_shares=[0.5, 0.25, 0.25])
    return blueprint.Blueprint(
        blueprint.Vertical(
            blueprint.Horizontal(overview, detail, column_shares=[0.68, 0.32]),
            blueprint.TextDocumentView(name="Legend and independent clocks", origin="/legend"),
            row_shares=[0.9, 0.1]),
        blueprint.TimePanel(timeline="arrival_index" if trace else "scene_index",
                            play_state="following", expanded=False),
        blueprint.BlueprintPanel(expanded=False), blueprint.SelectionPanel(expanded=False),
        auto_layout=False, auto_views=False)


def export_recording(output, scene, trace=None, report=None, images=None, visual_asset=None):
    rr, version = sdk()
    images = images or {}
    output = Path(output)
    require(output.suffix == ".rrd", "output must have .rrd suffix")
    require(not output.exists(), f"refusing to overwrite evidence: {output}")
    with tempfile.TemporaryDirectory(prefix=".rerun-", dir=output.parent) as directory:
        temporary = Path(directory)/"recording.rrd"
        screens = {screen["id"]: screen for screen in scene["screens"]} if scene else {
            screen["id"]: {**screen,
                           "marker_ids": [f"screen-{screen['id']}-marker-index-{i}" for i in range(6)]}
            for screen in trace["screens"]}
        recording = rr.RecordingStream("odyssey-tracking-regression")
        recording.save(temporary, default_blueprint=default_blueprint(scene, trace, screens))
        recording.log("world", rr.ViewCoordinates.RDF, static=True)
        recording.log("identity/runtime", rr.TextDocument(json.dumps({"rerun_sdk": version,
                      "world_axes": "X right, Y down, Z forward; meters",
                      "timing": "arrival and scene exposure clocks are separate; sensor timestamps are raw scalar metadata, never aligned timelines",
                      "image_model": "Pinhole frustums; raw distorted marker pixels stay unmodified. Rerun pinhole does not model OpenCV distortion."}, indent=2)), static=True)
        if visual_asset:
            transform = np.asarray(visual_asset["asset_to_world"])
            recording.log("world/visual_asset", rr.Transform3D(
                          translation=transform[:3, 3], mat3x3=transform[:3, :3]), static=True)
            recording.log("world/visual_asset", rr.Asset3D(path=visual_asset["path"]), static=True)
            recording.log("identity/visual_asset", rr.TextDocument(json.dumps(
                          {**visual_asset, "path": str(visual_asset["path"])}, indent=2)), static=True)
        recording.log("legend", rr.TextDocument(
            "GREEN: independent truth / reference. ORANGE: tracker estimate. BLUE: calibrated UV grid.\n"
            "Arrival_index drives observations, predictions and errors. Scene_index drives authored poses/images; "
            "select its clock explicitly. Clocks are NOT synchronized. Sensor timestamps and exposure are metadata.\n"
            "Source identity, physical/synthetic provenance and evaluation remain in metadata tab."), static=True)
        for screen in screens.values():
            base = f"world/screens/{screen['id']}"
            positions, triangles, uv, lines = screen_mesh(screen)
            recording.log(base+"/surface", rr.Mesh3D(vertex_positions=positions,
                          triangle_indices=triangles, vertex_texcoords=uv,
                          face_rendering=rr.components.MeshFaceRendering.DoubleSided,
                          vertex_colors=[[int(u*180)+50, int(v*180)+50, 120, 180] for u, v in uv]), static=True)
            recording.log(base+"/uv_grid", rr.LineStrips3D(lines, colors=[100, 180, 240]), static=True)
            markers = physical_to_world(screen, screen["calibration"]["object_points"])
            recording.log(base+"/markers", rr.Points3D(markers, labels=screen["marker_ids"],
                          radii=0.004, colors=[255, 255, 255], show_labels=False), static=True)
        if scene:
            recording.log("identity/scene", rr.TextDocument(scene["provenance"]), static=True)
            cameras = {camera["id"]: camera for camera in scene["cameras"]}
            paths = {}
            for camera in cameras.values():
                recording.log("identity/cameras/"+entity(camera["id"]), rr.TextDocument(json.dumps(camera, indent=2)), static=True)
                paths[camera["id"]] = []
            for index, frame in enumerate(scene["frames"]):
                set_clock(recording, "scene", index, frame["elapsed_us"])
                base = "world/reference/"+entity(frame["camera_id"])
                pose = frame["camera_to_world"]
                recording.log(base, rr.Transform3D(translation=pose["position_m"],
                              quaternion=pose["orientation_xyzw"]))
                camera = cameras[frame["camera_id"]]
                recording.log(base+"/image", rr.Pinhole(image_from_camera=camera["intrinsics"],
                              resolution=camera["image_size_px"], camera_xyz=rr.ViewCoordinates.RDF,
                              image_plane_distance=0.12))
                paths[frame["camera_id"]].append(pose["position_m"])
                recording.log("timing/scene/exposure_us", rr.Scalars(frame["exposure_us"]))
                recording.log("timing/scene/frame", rr.TextLog(json.dumps({key: frame[key] for key in
                              ("id", "camera_id", "elapsed_us", "exposure_us", "visible_marker_ids")})))
                log_hit(recording, rr, screens, frame["truth"], "reference/"+entity(frame["camera_id"]), TRUTH_COLOR)
                if frame["id"] in images:
                    recording.log(base+"/image", rr.EncodedImage(path=images[frame["id"]]))
            recording.reset_time()
            for camera_id, positions in paths.items():
                if len(positions) > 1:
                    recording.log("world/reference_trajectory/"+entity(camera_id),
                                  rr.LineStrips3D([positions], colors=TRUTH_COLOR), static=True)
        if trace:
            recording.log("identity/trace", rr.TextDocument(json.dumps({"name": trace["name"],
                          "provenance": trace["provenance"]}, indent=2)), static=True)
            for source in trace["sources"]:
                recording.log("identity/sources/"+entity(source["id"]),
                              rr.TextDocument(json.dumps(source_identity(trace, source), indent=2)), static=True)
                for kind in ("nf", "wf"):
                    model = source["settings"]["camera_model_"+kind]
                    intrinsics = np.asarray(model["k"]).reshape(3, 3, order="F")
                    recording.log("observed/"+entity(source["id"])+"/"+kind,
                                  rr.Pinhole(image_from_camera=intrinsics,
                                             camera_xyz=rr.ViewCoordinates.RDF), static=True)
            if report:
                recording.log("identity/evaluation", rr.TextDocument(json.dumps({key: value for key, value in report.items()
                              if key != "capture"}, indent=2)), static=True)
                recording.log("identity/capture", rr.TextDocument(json.dumps({key: value for key, value in report["capture"].items()
                              if key != "outputs"}, indent=2)), static=True)
            for index, sample in enumerate(trace["samples"]):
                set_clock(recording, "arrival", index, sample["elapsed_us"])
                source = entity(sample["source_id"])
                base = "samples/"+source
                recording.log(base+"/identity", rr.TextLog(json.dumps({key: sample[key] for key in
                              ("source_id", "elapsed_us", "expectation", "score", "image_artifacts")})))
                raw = sample["input"]
                recording.log(base+"/input", rr.TextLog(json.dumps(raw)))
                if "timestamp" in raw:
                    recording.log(base+"/sensor_clock/raw_timestamp_u32", rr.Scalars(raw["timestamp"]))
                if sample["expectation"] is not None:
                    recording.log(base+"/expected_valid", rr.Scalars(int(sample["expectation"])))
                for field, camera_kind in (("points", "nf"), ("nf_points", "nf"), ("wf_points", "wf")):
                    if field in raw:
                        points = [point for point in raw[field] if point != [0, 0]]
                        recording.log("observed/"+source+"/"+camera_kind+"/raw_pixels",
                                      rr.Points2D(np.asarray(points).reshape(-1, 2), colors=[255, 80, 180], radii=2))
                truth = sample["truth"]
                log_hit(recording, rr, screens, truth, "truth/"+source, TRUTH_COLOR)
                predicted = report["capture"]["outputs"][index] if report else None
                if predicted:
                    tracking = predicted["tracking"]
                    recording.log(base+"/predicted_valid", rr.Scalars(int(tracking is not None)))
                    recording.log(base+"/non_finite", rr.Scalars(int(predicted["non_finite"])))
                    log_hit(recording, rr, screens, tracking, "predicted/"+source, PREDICTED_COLOR)
                    pose_path = "world/predicted_pose/"+source
                    if tracking is None:
                        recording.log(pose_path, rr.Clear(recursive=True))
                    for error_name in ("aim_uv", "position_m", "screen_mismatch"):
                        recording.log(base+"/error/"+error_name, rr.Clear(recursive=False))
                    if tracking:
                        recording.log(pose_path, rr.Transform3D(translation=client_position(tracking["position_m"]),
                                      mat3x3=client_rotation(tracking["rotation"])))
                        recording.log(pose_path+"/axes", rr.Arrows3D(origins=np.zeros((3, 3)),
                                      vectors=np.eye(3)*0.3, colors=[[255, 0, 0], [0, 255, 0], [0, 0, 255]]))
                        recording.log(base+"/sensor_clock/output_timestamp_u32", rr.Scalars(tracking["timestamp"]))
                        if truth:
                            same_screen = tracking["screen_id"] == truth["screen_id"]
                            recording.log(base+"/error/screen_mismatch", rr.Scalars(int(not same_screen)))
                            if same_screen:
                                recording.log(base+"/error/aim_uv", rr.Scalars(float(np.linalg.norm(
                                              np.array(tracking["aimpoint"])-truth["aimpoint"]))))
                            if truth["position_m"] is not None:
                                recording.log(base+"/error/position_m", rr.Scalars(float(np.linalg.norm(
                                              np.array(tracking["position_m"])-truth["position_m"]))))
        recording.flush()
        recording.disconnect()
        # Hard-link is atomic and refuses concurrent output creation. SDK never opens evidence path.
        os.link(temporary, output)
    return output


def log_hit(recording, rr, screens, value, suffix, color):
    world_path, uv_path = "world/hits/"+suffix, "aim_uv/"+suffix
    recording.log(world_path, rr.Clear(recursive=True))
    recording.log(uv_path, rr.Clear(recursive=True))
    if value is None:
        return
    recording.log(uv_path, rr.Points2D([value["aimpoint"]], colors=color,
                  labels=[f"{suffix.split('/')[0]} screen={value['screen_id']}"], show_labels=False))
    screen = screens.get(value["screen_id"])
    if screen:
        point = uv_to_world(screen, value["aimpoint"])
        if point is not None:
            recording.log(world_path, rr.Points3D([point], colors=color, radii=0.025,
                          labels=[suffix.split("/")[0]], show_labels=False))


def read_recording(path, entities=None):
    import pyarrow as pa
    from rerun.chunk import RrdReader
    result = {}
    for chunk in RrdReader(path).stream():
        name = chunk.entity_path.lstrip("/")
        if entities is not None and name not in entities:
            continue
        batch = chunk.to_record_batch()
        columns = {}
        for field, column in zip(batch.schema, batch.columns):
            if field.name in ("log_time", "log_tick", "rerun.controls.RowId"):
                continue
            if pa.types.is_timestamp(field.type) or pa.types.is_duration(field.type):
                column = column.cast(pa.int64())
            columns[field.name] = column
        result.setdefault(name, []).extend(pa.table(columns).to_pylist())
    return result


def check_recording(path, scene, trace, report):
    selected = {"identity/runtime"}
    if scene:
        selected.update(f"world/screens/{screen['id']}/surface" for screen in scene["screens"])
    if report:
        selected.update("samples/"+entity(source["id"])+"/predicted_valid" for source in trace["sources"])
    rows = read_recording(path, selected)
    require("identity/runtime" in rows, "recording identity missing")
    if scene:
        for screen in scene["screens"]:
            values = rows[f"world/screens/{screen['id']}/surface"]
            actual = next(row["Mesh3D:vertex_positions"] for row in values if row.get("Mesh3D:vertex_positions"))
            np.testing.assert_allclose(actual, screen_mesh(screen)[0], rtol=1e-5, atol=1e-6)
    if report:
        for source in trace["sources"]:
            name = "samples/"+entity(source["id"])+"/predicted_valid"
            expected = [int(row["tracking"] is not None) for sample, row in
                        zip(trace["samples"], report["capture"]["outputs"]) if sample["source_id"] == source["id"]]
            actual = [row["Scalars:scalars"][0] for row in sorted(rows.get(name, []),
                       key=lambda row: row["arrival_index"])]
            require(actual == expected, "recorded prediction validity differs from evaluated capture")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="standalone scene v2 or original trace JSON")
    parser.add_argument("output", type=Path, help="new .rrd; never overwritten")
    predictions = parser.add_mutually_exclusive_group()
    predictions.add_argument("--capture", type=Path)
    predictions.add_argument("--report", type=Path, help="report.capture is re-evaluated, supplied metrics not trusted")
    parser.add_argument("--images", type=Path, help="Blender manifest with matching scene, images and optional digest-bound GLB")
    parser.add_argument("--validator", default="tracking-regression", help="authoritative Rust CLI executable")
    parser.add_argument("--self-check", action="store_true", help="read resulting .rrd through SDK Arrow API")
    args = parser.parse_args(argv)
    try:
        values = checked_inputs(args.input, args.capture, args.report, args.images, args.validator)
        export_recording(args.output, *values)
        if args.self_check:
            check_recording(args.output, values[0], values[1], values[2])
        print(json.dumps({"output": str(args.output), "sdk": importlib.metadata.version("rerun-sdk"),
                          "scene_frames": len(values[0]["frames"]) if values[0] else 0,
                          "trace_samples": len(values[1]["samples"]) if values[1] else 0,
                          "predictions_verified": values[2] is not None, "images": len(values[3]),
                          "visual_asset_verified": values[4] is not None}))
        return 0
    except (ValueError, KeyError, TypeError, OSError, subprocess.SubprocessError, RuntimeError) as error:
        print(f"rerun-export: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
