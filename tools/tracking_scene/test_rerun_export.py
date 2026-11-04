import copy
import json
import math
import os
from pathlib import Path
import tempfile
import subprocess
import unittest

import numpy as np

import rerun_export as export


def fixture():
    screen = {"id": 0, "calibration": {"rotation": [0, 0, 0, 1],
              "homography": [0.5, 0, 0, 0, 1, 0, 0, 0, 1],
              "object_points": [[0, 0, 0], [1, 0, 0], [2, 0, 0], [0, 1, 0], [1, 1, 0], [2, 1, 0]]},
              "marker_ids": [f"marker-{i}" for i in range(6)]}
    truth = {"aimpoint": [0.5, 0.5], "position_m": [1, -0.5, 1], "screen_id": 0}
    camera = {"id": "camera", "source_id": "physical/one", "image_size_px": [640, 480],
              "intrinsics": [[500, 0, 320], [0, 500, 240], [0, 0, 1]], "distortion": [0]*5}
    frame = {"id": "frame", "elapsed_us": 700, "exposure_us": 20, "camera_id": "camera",
             "camera_to_world": {"position_m": [1, 0.5, -1], "orientation_xyzw": [0, 0, 0, 1]},
             "visible_marker_ids": [], "truth": truth}
    scene = {"schema_version": 1, "provenance": "Synthetic fixture; not physical evidence",
             "coordinate_basis": "screen_x_right_y_down_z_forward", "screens": [screen],
             "cameras": [camera], "frames": [frame]}
    source = {"id": "physical/one", "device_uuid": [0, 1, 2, 3, 4, 5],
              "clock_basis": "physical arrival epoch unknown; sensor clock wraps independently",
              "settings": {"camera_model_nf": {"k": [500, 0, 0, 0, 500, 0, 320, 240, 1]},
                           "camera_model_wf": {"k": [500, 0, 0, 0, 500, 0, 320, 240, 1]}}}
    sample = {"elapsed_us": 900, "source_id": source["id"], "input": {"type": "imu", "timestamp": 4294967295,
              "accel": [0, 0, 9.81], "gyro": [0, 0, 0]}, "truth": truth,
              "expectation": True, "score": True, "image_artifacts": []}
    optical = copy.deepcopy(sample)
    optical["input"] = {"type": "poc", "points": [[23, 45], [0, 0], [0, 7]] + [[0, 0]]*13}
    missing = copy.deepcopy(sample)
    missing["elapsed_us"] = 1000
    missing["truth"] = None
    missing["input"]["timestamp"] = 9
    trace = {"name": "fixture", "provenance": {"description": "Physical device; independent synthetic truth fixture", "generator": None},
             "sources": [source], "screens": [{"id": 0, "calibration": screen["calibration"]}],
             "samples": [sample, optical, missing]}
    predicted = {"timestamp": 123, "aimpoint": [0.6, 0.5], "position_m": [1.1, -0.5, 1],
                 "rotation": [[0, -1, 0], [1, 0, 0], [0, 0, 1]], "distance_m": 1, "screen_id": 0}
    report = {"passed": True, "metrics": {"aim_rmse": 0.1}, "capture": {"backend": "test",
              "outputs": [{"sample_index": 0, "tracking": predicted, "non_finite": False},
                          {"sample_index": 1, "tracking": predicted, "non_finite": False},
                          {"sample_index": 2, "tracking": None, "non_finite": False}]}}
    return scene, trace, report


def recording_rows(path):
    return export.read_recording(path)


def component(rows, entity, fragment):
    return [value for row in rows[entity] for name, value in row.items()
            if fragment in name.lower() and value is not None and value != []]


class Geometry(unittest.TestCase):
    def test_client_rotation_changes_both_bases(self):
        rotation = [[0, -1, 0], [1, 0, 0], [0, 0, 1]]
        np.testing.assert_allclose(export.client_position([1, 2, 3]), [1, -2, -3])
        np.testing.assert_allclose(export.client_rotation(rotation), [[0, 1, 0], [-1, 0, 0], [0, 0, 1]])
        vector = np.array([0.4, -0.3, 0.2])
        np.testing.assert_allclose(export.client_rotation(rotation) @ export.client_position(vector),
                                   export.client_position(np.array(rotation) @ vector))

    def test_inverse_homography_is_projective_not_bilinear(self):
        scene, _, _ = fixture()
        screen = scene["screens"][0]
        screen["calibration"]["homography"] = [0.5, 0, 0.1, 0.1, 1, 0, 0, 0, 1]
        point = export.uv_to_world(screen, [0.4, 0.6])
        projected = np.asarray(screen["calibration"]["homography"]).reshape(3, 3, order="F") @ [point[0], point[1], 1]
        np.testing.assert_allclose(projected[:2]/projected[2], [0.4, 0.6])
        corners = [export.uv_to_world(screen, uv) for uv in ([0, 0], [1, 0], [0, 1], [1, 1])]
        bilinear = 0.6*0.4*corners[0]+0.4*0.4*corners[1]+0.6*0.6*corners[2]+0.4*0.6*corners[3]
        self.assertGreater(np.linalg.norm(point-bilinear), 0.01)
        screen["calibration"]["rotation"] = [0, 0, math.sqrt(0.5), math.sqrt(0.5)]
        np.testing.assert_allclose(export.physical_to_world(screen, [1, 0, 0]), [0, -1, 0], atol=1e-14)

    def test_wavy_grid_uses_cell_diagonal_and_nonlinear_spacing(self):
        scene, _, _ = fixture()
        screen = scene["screens"][0]
        screen["calibration"]["surface"] = {"columns": 2, "rows": 2,
                                            "positions_m": [[0, 0, 0], [2, 0, 0], [0, 1, 0], [3, 2, 4]]}
        np.testing.assert_allclose(export.uv_to_world(screen, [0.75, 0.25]), [1.75, 0.5, 1])
        np.testing.assert_allclose(export.uv_to_world(screen, [0.25, 0.75]), [0.75, 1, 1])
        np.testing.assert_allclose(export.uv_to_world(screen, [1, 1]), [3, 2, 4])
        _, triangles, _, _ = export.screen_mesh(screen)
        np.testing.assert_equal(triangles, [[0, 1, 3], [0, 3, 2]])
        self.assertIsNone(export.uv_to_world(screen, [1.01, 0.5]))

    def test_real_rrd_preserves_coordinates_clocks_identity_and_missing_outputs(self):
        scene, trace, report = fixture()
        source = export.entity(trace["sources"][0]["id"])
        with tempfile.TemporaryDirectory() as directory:
            path = export.export_recording(Path(directory)/"evidence.rrd", scene, trace, report)
            rows = recording_rows(path)
            vertices = component(rows, "world/screens/0/surface", "vertex_positions")[0]
            np.testing.assert_allclose(vertices[0], [0, 0, 0])
            np.testing.assert_allclose(vertices[-1], [2, 1, 0])
            pose_rows = rows["world/predicted_pose/"+source]
            translations = component(rows, "world/predicted_pose/"+source, "translation")
            np.testing.assert_allclose(translations[0][0], [1.1, 0.5, -1], atol=1e-6)
            clears = [row["arrival_index"] for row in pose_rows if "Clear:is_recursive" in row]
            self.assertEqual(clears, [2], "only the sample without tracking may clear the pose")
            validity = component(rows, "samples/"+source+"/predicted_valid", "scalar")
            self.assertEqual([row[0] for row in validity], [1, 1, 0])
            errors = component(rows, "samples/"+source+"/error/aim_uv", "scalar")
            np.testing.assert_allclose([row[0] for row in errors], [0.1, 0.1])
            sensor = component(rows, "samples/"+source+"/sensor_clock/raw_timestamp_u32", "scalar")
            self.assertEqual([row[0] for row in sensor], [4294967295, 9])
            for row in pose_rows:
                self.assertNotIn("scene_elapsed", row)
                self.assertNotIn("sensor_timestamp", row)
            reference_rows = rows["world/reference/"+export.entity("camera")]
            self.assertTrue(any("scene_elapsed" in row for row in reference_rows))
            self.assertFalse(any("arrival_elapsed" in row for row in reference_rows))
            observed = component(rows, "observed/"+source+"/nf/raw_pixels", "position")[0]
            self.assertEqual(observed, [[23, 45], [0, 7]])
            identity = component(rows, "identity/sources/"+source, "text")[0][0]
            metadata = json.loads(identity)
            self.assertEqual(metadata["device_uuid"], [0, 1, 2, 3, 4, 5])
            self.assertIsNone(metadata["trace_provenance"]["generator"])
            with self.assertRaisesRegex(ValueError, "overwrite"):
                export.export_recording(path, scene)

    def test_asset_rejects_changed_evidence_and_path_escape_before_import(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root/"visual.glb").write_bytes(b"changed asset")
            asset = {"path": "visual.glb", "sha256": "0"*64,
                     "asset_to_world": [[1, 0, 0, 0], [0, 0, -1, 0],
                                        [0, 1, 0, 0], [0, 0, 0, 1]]}
            with self.assertRaisesRegex(ValueError, "sha256 mismatch"):
                export.checked_asset(asset, root, "/not-invoked")
            asset["path"] = "../outside.glb"
            with self.assertRaisesRegex(ValueError, "escapes"):
                export.checked_asset(asset, root, "/not-invoked")

    def test_cbor_trace_capture_and_report_cross_rust_boundary(self):
        validator = os.environ.get("TRACKING_REGRESSION", "target/debug/tracking-regression")
        validator = str(Path(validator).resolve())
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run([validator, "generate", str(root/"fixtures")], check=True)
            trace_path = next((root/"fixtures").glob("*.cbor"))
            self.assertNotIn(trace_path.read_bytes()[:1], (b"{", b"["))
            trace = export.load_input(trace_path, validator)
            capture_path, report_path = root/"capture.cbor", root/"report.json"
            subprocess.run([validator, "capture", str(trace_path), str(capture_path)], check=True)
            self.assertNotIn(capture_path.read_bytes()[:1], (b"{", b"["))
            _, loaded, report, _, _ = export.checked_inputs(
                trace_path, capture_path, None, None, validator)
            self.assertEqual(loaded, trace)
            supplied = copy.deepcopy(report)
            supplied["passed"] = not report["passed"]
            report_path.write_text(json.dumps(supplied, allow_nan=False))
            _, _, rechecked, _, _ = export.checked_inputs(
                trace_path, None, report_path, None, validator)
            self.assertEqual(rechecked["passed"], report["passed"])
            self.assertEqual(rechecked["metrics"]["truth_matches"], report["metrics"]["truth_matches"])
            self.assertAlmostEqual(rechecked["metrics"]["aim_rmse"], report["metrics"]["aim_rmse"], places=12)
            self.assertAlmostEqual(rechecked["metrics"]["position_rmse_m"],
                                   report["metrics"]["position_rmse_m"], places=12)
            fake_binary = root/"fake.cbor"
            fake_binary.write_text(json.dumps(trace, allow_nan=False))
            with self.assertRaisesRegex(ValueError, "validator failed"):
                export.load_input(fake_binary, validator)
            json_trace = root/"trace.json"
            json_trace.write_text(json.dumps(trace, allow_nan=False))
            with self.assertRaisesRegex(ValueError, "traces must be CBOR"):
                export.load_input(json_trace, validator)

    def test_input_rejects_duplicate_keys_and_oversize_json(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)/"input.json"
            path.write_text('{"id":1,"id":2}')
            with self.assertRaisesRegex(ValueError, "duplicate"):
                export.load_json(path)
            with path.open("wb") as stream:
                stream.truncate(export.MAX_BYTES+1)
            with self.assertRaisesRegex(ValueError, "exceeds"):
                export.load_json(path)


if __name__ == "__main__":
    unittest.main()
