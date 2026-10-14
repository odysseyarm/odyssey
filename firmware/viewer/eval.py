#!/usr/bin/env python3
from io import TextIOWrapper
import json, sys, argparse, os
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("path", type=str, help="Path to directory containing calibration")
    args = parser.parse_args()
    path = Path(args.path)

    with open(path / "nearfield.json") as f:
        nf_calib = json.load(f)
    with open(path / "widefield.json") as f:
        wf_calib = json.load(f)
    with open(path / "stereo.json") as f:
        stereo_calib = json.load(f)

    outpath = path / "eval.html"
    print(f"Writing to {outpath}")
    with open(outpath, "w") as outfile:
        run(nf_calib, wf_calib, stereo_calib, outfile)

    print("file://" + str(Path(os.getcwd()) / outpath))


def run(nf_calib: dict, wf_calib: dict, stereo_calib: dict, outfile: TextIOWrapper):
    nf_calib_images = ['/'.join(p.split('/')[2:]) for p in nf_calib["image_files"]]
    wf_calib_images = ['/'.join(p.split('/')[2:]) for p in wf_calib["image_files"]]
    stereo_calib_images = {
        "wf": ['/'.join(p.split('/')[2:]) for p in stereo_calib["wf_image_files"]],
        "nf": ['/'.join(p.split('/')[2:]) for p in stereo_calib["nf_image_files"]],
    }

    outfile.write("<!DOCTYPE html><html><body>")
    outfile.write(f"""
    <div style="margin-bottom: 6pt">
        <select id="calib" onchange="console.log(this.value)">
            <option value="nf">Nearfield</option>
            <option value="wf">Widefield</option>
            <option value="stereo">Stereo</option>
        </select>
    </div>
    <div>
        <img id=wf_img width=500 height=500 />
        <img id=nf_img width=500 height=500 />
    </div>
    <div>
        <input
            style="width: 1000px"
            type="range"
            id="scrubber"
            min="0"
            value="0"
            step="1"
        />
        <span id="img_number">0</span>
    </div>
    <script>
        var nf_calib_images = {json.dumps(nf_calib_images)};
        var wf_calib_images = {json.dumps(wf_calib_images)};
        var stereo_calib_images = {json.dumps(stereo_calib_images)};

        var select = document.getElementById("calib");
        var scrubber = document.getElementById("scrubber");
        var wf_img = document.getElementById("wf_img");
        var nf_img = document.getElementById("nf_img");
        var img_number = document.getElementById("img_number");

        select.onchange = function(e) {{
            if (select.value == "nf") {{
                scrubber.max = nf_calib_images.length - 1;
            }} else if (select.value == "wf") {{
                scrubber.max = wf_calib_images.length - 1;
            }} else if (select.value == "stereo") {{
                scrubber.max = stereo_calib_images.wf.length - 1;
            }} else {{
                console.error("??? " + select.value);
            }}
            scrubber.value = 0;
            scrubber.oninput(null);
        }};
        scrubber.oninput = function(e) {{
            var i = scrubber.value;
            img_number.innerHTML = scrubber.value;
            if (select.value == "nf") {{
                wf_img.src = "";
                nf_img.src = nf_calib_images[i];
            }} else if (select.value == "wf") {{
                wf_img.src = wf_calib_images[i];
                nf_img.src = "";
            }} else if (select.value == "stereo") {{
                wf_img.src = stereo_calib_images.wf[i];
                nf_img.src = stereo_calib_images.nf[i];
            }} else {{
                console.error("??? " + select.value);
            }}
        }};

        select.onchange(null);
    </script>
""")

    outfile.write("</body></html>")


if __name__ == "__main__":
    main()
