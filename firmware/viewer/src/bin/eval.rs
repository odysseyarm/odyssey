use std::io::Write;
use std::path::PathBuf;
use std::{env, fs};

use eros::Context as _;
use serde_json::Value;

fn main() -> std::process::ExitCode {
    match run_application().context("Calibration evaluation export failed") {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#?}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run_application() -> eros::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <path>", args[0]);
        std::process::exit(1);
    }

    let path = PathBuf::from(&args[1]);

    let nf_calib = read_json(path.join("nearfield.json"))?;
    let wf_calib = read_json(path.join("widefield.json"))?;
    let stereo_calib = read_json(path.join("stereo.json"))?;

    let outpath = path.join("eval.html");
    println!("Writing to {:?}", outpath);

    let mut outfile = fs::File::create(&outpath).context("Unable to create evaluation output")?;
    run(&nf_calib, &wf_calib, &stereo_calib, &mut outfile)?;

    println!("file://{}", outpath.canonicalize()?.display());
    Ok(())
}

fn read_json(path: PathBuf) -> eros::Result<Value> {
    let data = fs::read_to_string(&path)
        .with_context(|| format!("Unable to read calibration {}", path.display()))?;
    Ok(serde_json::from_str(&data)
        .with_context(|| format!("Unable to parse calibration {}", path.display()))?)
}

fn image_files(calibration: &Value, field: &str) -> eros::Result<Vec<String>> {
    let files = calibration[field]
        .as_array()
        .ok_or_else(|| eros::error!("Calibration field {field} must be an array"))?;
    files
        .iter()
        .map(|path| {
            let path = path
                .as_str()
                .ok_or_else(|| eros::error!("Calibration field {field} must contain file paths"))?;
            Ok(path.split('/').skip(2).collect::<Vec<_>>().join("/"))
        })
        .collect()
}

fn run(
    nf_calib: &Value,
    wf_calib: &Value,
    stereo_calib: &Value,
    outfile: &mut fs::File,
) -> eros::Result<()> {
    let nf_calib_images = image_files(nf_calib, "image_files")?;
    let wf_calib_images = image_files(wf_calib, "image_files")?;
    let stereo_calib_images = serde_json::json!({
        "wf": image_files(stereo_calib, "wf_image_files")?,
        "nf": image_files(stereo_calib, "nf_image_files")?,
    });

    let html_content = format!(
        r#"<!DOCTYPE html>
<html>
<body>
    <div style="margin-bottom: 6pt">
        <select id="calib" onchange="console.log(this.value)">
            <option value="nf">Nearfield</option>
            <option value="wf">Widefield</option>
            <option value="stereo">Stereo</option>
        </select>
    </div>
    <div>
        <img id="wf_img" width="500" height="500" />
        <img id="nf_img" width="500" height="500" />
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
        var nf_calib_images = {nf_calib_images};
        var wf_calib_images = {wf_calib_images};
        var stereo_calib_images = {stereo_calib_images};

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
</body>
</html>
"#,
        nf_calib_images = serde_json::to_string(&nf_calib_images)?,
        wf_calib_images = serde_json::to_string(&wf_calib_images)?,
        stereo_calib_images = serde_json::to_string(&stereo_calib_images)?
    );

    outfile
        .write_all(html_content.as_bytes())
        .context("Unable to write evaluation HTML")?;
    Ok(())
}
