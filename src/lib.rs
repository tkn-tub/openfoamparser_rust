extern crate nalgebra as na;

use std::io;
use std::path::Path;
use std::collections::HashMap;
use na::{geometry::Point3, Vector3};


pub struct FoamMesh {
    // pub boundary: ???,
    pub points: Vec<Point3<f64>>,
    // pub faces: ???,
    // pub owner: ???,
    // pub neighbor: ???,
    pub cell_centers: Vec<Point3<f64>>
    // pub cell_volumes: ???,
    // pub face_areas: ???
}

#[derive(Debug)]
pub struct Boundary {
    pub boundary_type: String,
    num_faces: usize,
    start_face: usize,
    boundary_id: i64,  // original implementation seems to allow neg. values
}

impl FoamMesh {
    pub fn new(path: &Path) -> Result<FoamMesh, &str> {
        // TODO
        Ok(FoamMesh {
            points: Vec::new(),
            cell_centers: Vec::new()
        })
    }

    /// Parse an OpenFOAM boundary definition.
    ///
    /// Expects a file in the following format:
    /// ```plaintext
    /// 3
    /// (
    ///     inlet
    ///     {
    ///         type            patch;
    ///         nFaces          605;
    ///         startFace       971201;
    ///     }
    ///     outlet
    ///     {
    ///         type            patch;
    ///         nFaces          605;
    ///         startFace       971806;
    ///     }
    ///     walls
    ///     {
    ///         type            patch;
    ///         nFaces          23848;
    ///         startFace       972411;
    ///     }
    /// )
    /// ```
    fn parse_boundary(
        filename: &Path,
        skip: usize
    ) -> Result<HashMap<String, Boundary>, std::io::Error> {
        // TODO: This, like the reference implementation, relies an
        //  awful lot on an expected number of newlines between elements…
        fn get_val(line: &str) -> Result<&str, std::io::Error> {
            if let Some(val_str) = line.split(' ')
                    .filter(|s| !s.is_empty()).nth(1) {
                if let Some(val_str) = val_str.strip_suffix(";") {
                    return Ok(val_str)
                }
            }
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Malformed key-value pair in boundary definition: '{}'",
                    line
                )
            ))
        }
        fn get_parsed_val<T: std::str::FromStr>(
            line: &str
        ) -> Result<T, std::io::Error> {
            match get_val(line)?.parse::<T>() {
                Ok(val) => {
                    Ok(val)
                },
                Err(_) => {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Malformatted boundary data: \"{}\"", line)
                    ))
                }
            }
        }

        let content: Vec<String> = std::fs::read_to_string(&filename)?
            .split('\n')
            .skip(skip)
            .map(|l| String::from(l))
            .collect(); // TODO: rewrite loop below for single pass
        let mut bd: HashMap<String, Boundary> = HashMap::new();
        let mut in_boundary_field = false;
        let mut in_patch_field = false;
        let mut current_patch: String = String::from("");
        let mut current_type: String = String::from("");
        let mut current_num_faces: usize = 0;
        let mut current_start_face: usize = 0;
        let mut bid: i64 = 0; // TODO: can this really be <0?

        let mut i: usize = 0;
        loop {
            if i > content.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Reached end of file unexpectedly. \
                    Missing closing bracket?"
                ));
            }
            let line = content[i].clone();
            if !in_boundary_field {
                if let Ok(_) = line.trim().parse::<i64>() {
                    in_boundary_field = true;
                    if content[i+1].starts_with('(') {
                        i += 2;
                        continue;
                    } else if content[i+1].trim().is_empty()
                            && content[i+2].starts_with('(') {
                        i += 3;
                        continue;
                    } else {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Missing '(' after number of boundaries"
                        ));
                    }
                }
            }
            if in_boundary_field {
                if line.starts_with(')') { break; }
                if in_patch_field {
                    if line.trim() == "}" {
                        in_patch_field = false;
                        bd.insert(current_patch, Boundary{
                            boundary_type: current_type.clone(),
                            num_faces: current_num_faces,
                            start_face: current_start_face,
                            boundary_id: bid
                        });
                        bid += 1;
                        current_patch = String::from("");
                    } else if line.contains("nFaces") {
                        // example: "        nFaces          605;"
                        current_num_faces = get_parsed_val(&line)?;
                    } else if line.contains("startFace") {
                        current_start_face = get_parsed_val(&line)?;
                    } else if line.contains("type") {
                        current_type = String::from(get_val(&line)?);
                    }
                } else { // not in_patch_field
                    if line.trim().is_empty() {
                        i += 1;
                        continue;
                    }
                    current_patch = String::from(line.trim());
                    if content[i+1].trim() == "{" {
                        i += 2;
                    } else if content[i+1].trim().is_empty()
                            && content[i+2].trim() == "{" {
                        i += 3;
                    } else {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Missing '{' after boundary patch"
                        ));
                    }
                    in_patch_field = true;
                    continue;
                }
            }
            i += 1;
        }

        Ok(bd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_parse_boundary() {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("resources/test/cavity/constant/polyMesh/boundary");
        let boundaries: HashMap<String, Boundary> = FoamMesh::parse_boundary(
            d.as_path(),
            10 // default skip…
        ).unwrap();
    }
}
