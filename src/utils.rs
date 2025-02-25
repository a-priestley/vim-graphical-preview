use nix::{ioctl_read_bad, pty::Winsize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::{io::Write, str, usize};

use crate::error::{Error, Result};
use crate::render::ART_PATH;

pub fn hash(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    let mut x = format!("{:x}", &result);
    x.truncate(24);
    x
}

/// Get pixel height of a character
pub fn char_pixel_height() -> usize {
    ioctl_read_bad! { tiocgwinsz, 21523, Winsize }

    let mut size = Winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    unsafe { tiocgwinsz(0, &mut size).unwrap() };

    if size.ws_ypixel > 2 {
        size.ws_ypixel as usize / size.ws_row as usize
    } else {
        28
    }
}

/// Generate SVG file from latex file with given zoom
pub fn generate_svg_from_latex(path: &Path, zoom: f32) -> Result<PathBuf> {
    let dest_path = path.parent().unwrap();
    let file: &Path = path.file_name().unwrap().as_ref();

    // use latex to generate a dvi
    let dvi_path = path.with_extension("dvi");
    if !dvi_path.exists() {
        let latex_path = which::which("latex").map_err(Error::BinaryNotFound)?;

        let cmd = Command::new(latex_path)
            .current_dir(&dest_path)
            //.arg("--jobname").arg(&dvi_path)
            .arg(&file.with_extension("tex"))
            .output()
            .expect("Could not spawn latex");

        if !cmd.status.success() {
            let buf = String::from_utf8_lossy(&cmd.stdout);

            // latex prints error to the stdout, if this is empty, then something is fundamentally
            // wrong with the latex binary (for example shared library error). In this case just
            // exit the program
            if buf.is_empty() {
                let buf = String::from_utf8_lossy(&cmd.stderr);
                panic!("Latex exited with `{}`", buf);
            }

            let err = buf
                .split('\n')
                .filter(|x| {
                    (x.starts_with("! ") || x.starts_with("l.")) && !x.contains("Emergency stop")
                })
                .fold(("", "", usize::MAX), |mut err, elm| {
                    if elm.starts_with("! ") {
                        err.0 = elm;
                    } else if let Some(elms) = elm.strip_prefix("1.") {
                        let mut elms = elms.splitn(2, ' ').map(|x| x.trim());
                        if let Some(Ok(val)) = elms.next().map(|x| x.parse::<usize>()) {
                            err.2 = val;
                        }
                        if let Some(val) = elms.next() {
                            err.1 = val;
                        }
                    }

                    err
                });

            return Err(Error::InvalidMath(
                err.0.to_string(),
                err.1.to_string(),
                err.2,
            ));
        }
    }

    // convert the dvi to a svg file with the woff font format
    let svg_path = path.with_extension("svg");
    if !svg_path.exists() && dvi_path.exists() {
        let dvisvgm_path = which::which("dvisvgm").map_err(Error::BinaryNotFound)?;

        let cmd = Command::new(dvisvgm_path)
            .current_dir(&dest_path)
            .arg("-b")
            .arg("1")
            //.arg("--font-format=woff")
            .arg("--no-fonts")
            .arg(&format!("--zoom={}", zoom))
            .arg(&dvi_path)
            .output()
            .expect("Couldn't run svisvgm properly!");

        let buf = String::from_utf8_lossy(&cmd.stderr);
        if !cmd.status.success() || buf.contains("error:") {
            return Err(Error::InvalidDvisvgm(buf.to_string()));
        }
    }

    Ok(path.to_path_buf())
}

/// Parse an equation with the given zoom
pub fn parse_equation(content: &str, zoom: f32) -> Result<PathBuf> {
    let path = Path::new(ART_PATH)
        .join(hash(content))
        .with_extension("svg");

    // create a new tex file containing the equation
    if !path.with_extension("tex").exists() {
        let mut file = File::create(path.with_extension("tex")).map_err(Error::Io)?;

        file.write_all("\\documentclass[20pt, preview]{standalone}\n\\usepackage{amsmath}\\usepackage{amsfonts}\n\\begin{document}\n$$\n".as_bytes())
            .map_err(Error::Io)?;

        file.write_all(content.as_bytes()).map_err(Error::Io)?;

        file.write_all("$$\n\\end{document}".as_bytes())
            .map_err(Error::Io)?;
    }

    generate_svg_from_latex(&path, zoom)
}

/// Generate latex file from gnuplot
///
/// This function generates a latex file with gnuplot `epslatex` backend and then source it into
/// the generate latex function
pub fn generate_latex_from_gnuplot(content: &str) -> Result<PathBuf> {
    let path = Path::new(ART_PATH)
        .join(hash(content))
        .with_extension("tex");

    let gnuplot_path = which::which("gnuplot").map_err(Error::BinaryNotFound)?;

    let cmd = Command::new(gnuplot_path)
        .stdin(Stdio::piped())
        .current_dir(ART_PATH)
        .arg("-p")
        .spawn()
        .unwrap();
    //.expect("Could not spawn gnuplot");

    let mut stdin = cmd.stdin.unwrap();

    stdin
        .write_all(
            format!(
                "set output '{}'\n",
                path.file_name().unwrap().to_str().unwrap()
            )
            .as_bytes(),
        )
        .map_err(Error::Io)?;
    stdin
        .write_all("set terminal epslatex color standalone\n".as_bytes())
        .map_err(Error::Io)?;
    stdin.write_all(content.as_bytes()).map_err(Error::Io)?;

    Ok(path)
}

pub fn generate_latex_from_gnuplot_file(path: &Path) -> Result<PathBuf> {
    let mut content = String::new();
    let mut f = File::open(path).map_err(Error::Io)?;
    f.read_to_string(&mut content).unwrap();

    let path = generate_latex_from_gnuplot(&content)?;
    generate_svg_from_latex(&path, 1.0)
}

/// Parse a latex content and convert it to a SVG file
pub fn parse_latex(content: &str) -> Result<PathBuf> {
    let path = Path::new(ART_PATH)
        .join(hash(content))
        .with_extension("svg");

    // create a new tex file containing the equation
    if !path.with_extension("tex").exists() {
        let mut file = File::create(&path.with_extension("tex")).map_err(Error::Io)?;

        file.write_all(content.as_bytes()).map_err(Error::Io)?;
    }

    if !path.exists() {
        generate_svg_from_latex(&path, 1.0)?;
    }

    Ok(path)
}

pub fn parse_latex_from_file(path: &Path) -> Result<PathBuf> {
    let mut content = String::new();
    let mut f = File::open(path).map_err(Error::Io)?;
    f.read_to_string(&mut content).unwrap();

    parse_latex(&content)
}
