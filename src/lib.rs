// openfoamparser
// Copyright (C) 2020 Data Communications and Networking (TKN), TU Berlin
//
// This file is part of openfoamparser.
//
// openfoamparser is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// openfoamparser is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with Pogona.  If not, see <https://www.gnu.org/licenses/>.

//! OpenFOAM Parser
//!
//! openfoamparser lets you parse OpenFOAM simulation results just
//! like the Python library [openfoamparser](https://github.com/ApolloLV/openfoamparser.git).
//!
//! Known limitations:
//! - Parsing binary files is not supported yet.
//!
//! # Getting Started
//!
//! The following example loads an existing vector field:
//!
//! ```
//! extern crate nalgebra as na;
//! use std::path::PathBuf;
//! use na::{Vector3, Point3};
//!
//! use openfoamparser as ofp;
//!
//! let d: PathBuf = [
//!     env!("CARGO_MANIFEST_DIR"),
//!     "resources/test/cavity/"
//! ].iter().collect();
//!
//! // Load the mesh (and nothing else):
//! let mut fm = ofp::FoamMesh::new(&d).unwrap();
//!
//! // Load the cell centers from time step 0.5 s.
//! // This requires that the following or a similar command has been run:
//! // `runApplication postProcess -func writeCellCentres -latestTime`
//! fm.read_cell_centers(d.join("0.5/C")).unwrap();
//!
//! // Load the flow speeds from the same time step:
//! let flow: Vec<Vector3<f64>> = ofp::parse_internal_field(
//!     fm.path.join("0.5/U"),
//!     |s| ofp::parse_vector3(s)
//! ).unwrap();
//!
//! // …
//! ```

extern crate nalgebra as na;
extern crate regex;

#[macro_use]
extern crate lazy_static;
#[cfg_attr(test, macro_use)]
extern crate approx;

use std::io;
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use na::{geometry::Point3, Vector3};
use regex::Regex;

pub struct FoamMesh {
    pub path: PathBuf,
    pub boundary: HashMap<String, Boundary>,
    pub points: Vec<Point3<f64>>,
    /// A face is defined as a list of point indices.
    /// Each face also is represented in the list of
    /// owners and neighbors.
    pub faces: Vec<Vec<usize>>,
    pub cell_faces: Vec<Vec<usize>>,
    /// Indices of the cell that is the owner of the
    /// respective face.
    pub owners: Vec<usize>,
    /// Indices of neighboring cells for each internal
    /// face.
    pub neighbors: Vec<i64>,
    pub cell_neighbors: Vec<Vec<i64>>,
    pub cell_centers: Option<Vec<Point3<f64>>>,
    num_inner_faces: usize,
    num_cells: usize,
    // pub cell_volumes: ???,
    // pub face_areas: ???
}

#[derive(Debug)]
pub struct Boundary {
    pub boundary_type: String,
    pub num_faces: usize,
    pub start_face: usize,
    pub boundary_id: i64,  // original implementation seems to allow neg. values
}

impl FoamMesh {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<FoamMesh, io::Error> {
        let mut pb: PathBuf = PathBuf::new();
        pb.push(&path);
        pb.push("constant/polyMesh/");

        let boundary: HashMap<String, Boundary> = FoamMesh::parse_boundary(
            &pb.join("boundary"), 10)?;
        let faces: Vec<Vec<usize>> = FoamMesh::parse_faces(&pb.join("faces"), 10)?;
        let owners: Vec<usize> = FoamMesh::parse_scalars(&pb.join("owner"), 10)?;
        let mut neighbors: Vec<i64> = FoamMesh::parse_scalars(
            &pb.join("neighbour"), 10)?; // OpenFoam uses the British spelling

        let num_faces = owners.len();
        let num_inner_faces = neighbors.len();
        let num_cells: usize = *owners.iter().max().unwrap();

        // _set_boundary_faces:
        neighbors.extend(vec![-10; num_faces - num_inner_faces]);
        for b in boundary.values() {
            for i in b.start_face .. b.start_face + b.num_faces {
                neighbors[i] = b.boundary_id;
            }
        }

        // _construct_cells:
        let cell_num: usize = std::cmp::max(
            num_cells as i64,
            *neighbors.iter().max().unwrap()
        ) as usize + 1;
        let mut cell_faces: Vec<Vec<usize>> = vec![Vec::new(); cell_num];
        let mut cell_neighbors: Vec<Vec<i64>> = vec![Vec::new(); cell_num];
        for (i, &owner) in owners.iter().enumerate() {
            cell_faces[owner].push(i);
        }
        for (i, &neighbor) in neighbors.iter().enumerate() {
            if neighbor >= 0 {
                cell_faces[neighbor as usize].push(i);
                cell_neighbors[neighbor as usize].push(owners[i] as i64);
            }
            cell_neighbors[owners[i]].push(neighbor);
        }

        Ok(FoamMesh {
            path: PathBuf::new().join(&path),
            boundary,
            points: FoamMesh::parse_points(&pb.join("points"), 10)?,
            faces,
            cell_faces,
            owners,
            neighbors,
            cell_neighbors,
            num_inner_faces,
            num_cells,
            cell_centers: None
        })
    }

    /// Read cell center coordinates from the given file
    /// (e.g., `0/C`).
    ///
    /// Such a file can be generated by running
    /// `postProcess -func 'writeCellCentres' -time 0`.
    pub fn read_cell_centers<P: AsRef<Path>>(
        &mut self, filename: P
    ) -> Result<(), io::Error> {
        self.cell_centers = Some(parse_internal_field(
            filename,
            |s| parse_point3(s)
        )?);
        Ok(())
    }

    pub fn num_inner_faces(&self) -> usize {
        return self.num_inner_faces;
    }

    pub fn num_cells(&self) -> usize {
        return self.num_cells;
    }

    /// Return the indices of neighbor cells of the cell with index `cell_id`.
    pub fn cell_neighbor_cells(&self, cell_id: usize) -> Option<&Vec<i64>> {
        self.cell_neighbors.get(cell_id)
    }

    /// Check if a cell is on a boundary.
    ///
    /// Run-time complexity is in O(n), where n is the maximum number of
    /// neighbors of a cell.
    pub fn is_cell_on_boundary(
        &self,
        cell_id: usize,
        bd_name: Option<String>
    ) -> bool {
        if cell_id >= self.num_cells { return false; }
        let mut bid: i64 = 0;
        if let Some(bd_name) = &bd_name {
            if let Some(bd) = self.boundary.get(bd_name) {
                bid = bd.boundary_id;
            } else {
                return false;
            }
        }
        for &neighbor in self.cell_neighbors[cell_id].iter() {
            if bd_name == None && neighbor < 0 {
                return true;
            } else if bd_name != None && neighbor == bid {
                return true;
            }
        }
        false
    }

    /// Check if a face is a boundary face (in O(1)).
    pub fn is_face_on_boundary(
        &self,
        face_id: usize,
        bd_name: Option<String>
    ) -> bool {
        if face_id >= self.faces.len() { return false; }
        if let Some(bd_name) = &bd_name {
            if let Some(bd) = self.boundary.get(bd_name) {
                return self.neighbors[face_id] == bd.boundary_id;
            } else {
                false
            }
        } else {
            return self.neighbors[face_id] < 0;
        }
    }

    /// Get cell IDs of cells on a given boundary.
    /// Returns an empty vector if the named boundary does not exist.
    pub fn boundary_cells(&self, bd_name: &str) -> Vec<usize> {
        if let Some(bd) = self.boundary.get(bd_name) {
            (bd.start_face .. bd.start_face + bd.num_faces)
                .map(|face_id| self.owners[face_id])
                .collect()
        } else { vec![] }
    }

    /// Parse scalar values from a given ASCII file.
    ///
    /// Expects a file in the following format:
    /// ```plaintext
    /// // …
    ///
    /// 11360
    /// (
    /// 42
    /// 0
    /// 3
    /// // …
    /// )
    /// ```
    pub fn parse_scalars<P: AsRef<Path>, T: std::str::FromStr>(
        filename: P,
        skip: usize
    ) -> Result<Vec<T>, io::Error> {
        let mut data: Vec<T> = Vec::new();
        let mut num_expected: usize = 0;
        for line in read_to_string(&filename)?
                .split('\n').skip(skip) {
            if num_expected > 0 {
                if let Ok(val) = line.parse::<T>() {
                    data.push(val);
                }
            } else if let Ok(num_values) = line.parse::<usize>() {
                num_expected = num_values;
            }
        }
        if data.len() != num_expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} values expected, but parsed {}.",
                    num_expected,
                    data.len()
                )
            ));
        }
        Ok(data)
    }

    /// Parse faces from a given ASCII file.
    /// Each face is a list of point indices.
    ///
    /// Expects a file in the following format:
    /// ```plaintext
    /// // …
    ///
    /// 11360
    /// (
    /// 4(1 42 1723 1682)
    /// 3(2 3 4)
    /// // …
    /// )
    /// ```
    pub fn parse_faces<P: AsRef<Path>>(
        filename: P,
        skip: usize
    ) -> Result<Vec<Vec<usize>>, io::Error> {
        lazy_static! {
            static ref RE_NUM: Regex = Regex::new(
                r"\d+"
            ).unwrap();
        }

        let mut data: Vec<Vec<usize>> = Vec::new();
        let mut num_faces_expected: usize = 0;
        for (i, line) in read_to_string(&filename)?
                .split('\n')
                .skip(skip)
                .enumerate() {
            if num_faces_expected > 0 {
                // We already encountered the initial line stating
                // the number of expected faces.
                // Now read the actual data.
                let mut vals: Vec<usize> = RE_NUM.captures_iter(&line)
                    .map(|cap| cap[0].parse::<usize>().unwrap())
                    .collect();
                if vals.len() == 0 { continue; }
                if vals.len() != vals[0] + 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "Malformed faces file, l. {} (\"{}\"): \
                            Mismatch between number of vertices announced \
                            and found.",
                            skip+i,
                            line
                        )
                    ));
                }
                vals.remove(0);
                data.push(vals);
            } else if let Ok(num_faces) = line.parse::<usize>() {
                num_faces_expected = num_faces;
            }
        }
        if data.len() != num_faces_expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} faces expected, but parsed {}.",
                    num_faces_expected,
                    data.len()
                )
            ));
        }
        Ok(data)
    }

    /// Parse mesh point data from a given ASCII file.
    ///
    /// Expects a file in the following format:
    /// ```plaintext
    /// // …
    ///
    /// 5043
    /// (
    /// (42 0 1)
    /// (3 2.001 13.37)
    /// // …
    /// )
    /// ```
    pub fn parse_points<P: AsRef<Path>>(
        filename: P,
        skip: usize
    ) -> Result<Vec<Point3<f64>>, io::Error> {
        let mut num_points_expected: usize = 0;
        let mut data: Vec<Point3<f64>> = Vec::new();
        for (i, line) in read_to_string(&filename)?
                .split('\n')
                .skip(skip)
                .enumerate() {
            if num_points_expected > 0 {
                // We already encountered the initial line stating
                // the number of expected points.
                // Now read the actual data.
                if !line.starts_with('(') || !line.ends_with(')') {
                    continue;
                }
                if let Some(v) = parse_point3(line) {
                    data.push(v);
                } else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "Malformed points file, l. {} (\"{}\"): \
                            Could not parse three floats.",
                            skip+i,
                            line
                        )
                    ));
                }
            } else if let Ok(num_points) = line.parse::<usize>() {
                num_points_expected = num_points;
            }
        }
        if data.len() != num_points_expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} points expected, but parsed {}.",
                    num_points_expected,
                    data.len()
                )
            ));
        }
        Ok(data)
    }

    /// Parse an OpenFOAM boundary definition file.
    ///
    /// Expects a file in the following format:
    /// ```plaintext
    /// // …
    ///
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
    pub fn parse_boundary<P: AsRef<Path>>(
        filename: P,
        skip: usize
    ) -> Result<HashMap<String, Boundary>, std::io::Error> {
        // TODO: This, like the reference implementation, relies an
        //  awful lot on an expected number of newlines between elements…
        fn get_val(line: &str) -> Result<&str, std::io::Error> {
            // example: "        nFaces          605;" -> "605"
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

        let content: Vec<String> = read_to_string(&filename)?
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
                            boundary_id: -10-bid // TODO: why? In Python impl, _set_boundary_faces, -10 seems to be default neighbor for boundaries…
                        });
                        bid += 1;
                        current_patch = String::from("");
                    } else if line.contains("nFaces") {
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

/// Parse an internal field such as a cell centers file.
///
/// Expects a closure `parse_fn` to parse a single value to
/// the desired type (e.g., "(0.1 0 3.3)" to a Vector3).
///
/// If the internal field is declared 'nonuniform', this function
/// will only read the first section (for some reason), such as to be
/// compatible with the reference Python implementation for now.
///
/// Similarly, if the internal field is declared 'uniform',
/// only the first data line will be read.
pub fn parse_internal_field<T, P, F>(
    filename: P,
    parse_fn: F
) -> Result<Vec<T>, io::Error> where
        P: AsRef<Path>,
        F: Fn(&str) -> Option<T> {
    let content: Vec<String> = read_to_string(&filename)?
            .split('\n')
            .map(|s| String::from(s))
            .collect();
    for (i, line) in content.iter().enumerate() {
        if !line.starts_with("internalField") { continue; }
        if line.contains("nonuniform") {
            return parse_internal_field_data_nonuniform(
                &content,
                i,
                content.len(),
                parse_fn
            );
        } else if line.contains("uniform") {
            return parse_internal_field_data_uniform(
                line,
                parse_fn
            );
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Malformed internal field file: Not defined as either \
            uniform of nonuniform."
        ));
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "Did not find any data in internal field file."
    ))
}

/// Parse uniform data from a line.
///
/// Example input line:
/// ```plaintext
/// value           uniform (0 0 0);
/// ```
fn parse_internal_field_data_uniform<T, F>(
    line: &str,
    parse_fn: F
) -> Result<Vec<T>, io::Error> where
        F: Fn(&str) -> Option<T> {
    let start = line.find('(');
    let end = line.find(')');
    if let (Some(start), Some(end)) = (start, end) {
        Ok(line[start+1..end]
             .split(' ')
             .filter_map(|s| parse_fn(s))
             .collect()
        )
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Malformed internal field uniform data line:\n{}",
                line
            )
        ))
    }
}

fn parse_internal_field_data_nonuniform<T, F>(
    content: &[String],
    start: usize,
    _end: usize, // only needed for binary, not implemented yet
    parse_fn: F
) -> Result<Vec<T>, io::Error> where
        F: Fn(&str) -> Option<T> {
    if let Ok(num_vals_expected) = content[start+1].parse::<usize>() {
        if num_vals_expected + start > content.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Internal field file is shorter than declared."
            ));
        }
        let mut data: Vec<T> = Vec::new();
        data.reserve_exact(num_vals_expected);
        for line in &content[start+3..start+3+num_vals_expected] {
            if let Some(val) = parse_fn(line) {
                data.push(val);
            }
        }
        if data.len() != num_vals_expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} values expected, but parsed {}.",
                    num_vals_expected,
                    data.len()
                )
            ));
        }
        Ok(data)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Malformed internal field file: Number of expected \
            values not given."
        ))
    }
}

fn parse_vals_from_brackets<T: std::str::FromStr>(s: &str) -> Option<Vec<T>> {
    Some(s.strip_prefix("(")?
        .strip_suffix(")")?
        .split(' ')
        .filter_map(|s| s.parse::<T>().ok())
        .collect())
}

pub fn parse_point3<T>(s: &str) -> Option<Point3<T>> where
        T: std::fmt::Debug + Copy + PartialEq + std::str::FromStr + 'static {
    let vals = parse_vals_from_brackets(s)?;
    if vals.len() != 3 { return None; }
    Some(Point3::new(vals[0], vals[1], vals[2]))
}

pub fn parse_vector3<T>(s: &str) -> Option<Vector3<T>> where
        T: std::fmt::Debug + Copy + PartialEq + std::str::FromStr + 'static {
    let vals = parse_vals_from_brackets(s)?;
    if vals.len() != 3 { return None; }
    Some(Vector3::new(vals[0], vals[1], vals[2]))
}

fn read_to_string<P: AsRef<Path>>(path: P) -> Result<String, io::Error> {
    match std::fs::read_to_string(&path) {
        Err(e) => Err(io::Error::new(
            e.kind(),
            format!(
                "Could not read \"{}\": {}",
                path.as_ref().to_string_lossy(),
                e.to_string()
            )
        )),
        Ok(s) => Ok(s)
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
            d,
            10 // default skip…
        ).unwrap();
        let bd_fixed_wall = boundaries.get("fixedWalls").unwrap();
        assert_eq!(bd_fixed_wall.boundary_type, "wall");
        assert_eq!(bd_fixed_wall.num_faces, 240);
        assert_eq!(bd_fixed_wall.start_face, 7920);
    }

    #[test]
    fn test_parse_points() {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("resources/test/cavity/constant/polyMesh/points");
        let points: Vec<Point3<f64>> = FoamMesh::parse_points(
            d,
            10 // default skip…
        ).unwrap();
        assert_relative_eq!(points[0], Point3::new(0_f64, 0_f64, 0_f64));
        assert_relative_eq!(
            points[5042],
            Point3::new(0.1_f64, 0.1_f64, 0.01_f64)
        );
    }

    #[test]
    fn test_parse_faces() {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("resources/test/cavity/constant/polyMesh/faces");
        let faces: Vec<Vec<usize>> = FoamMesh::parse_faces(
            d,
            10 // default skip…
        ).unwrap();
        assert_eq!(faces[0], vec![1, 42, 1723, 1682]);
    }

    #[test]
    fn test_parse_scalars() {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let owners: Vec<usize> = FoamMesh::parse_scalars(
            d.join("resources/test/cavity/constant/polyMesh/owner"),
            10 // default skip…
        ).unwrap();
        assert_eq!(owners[0], 0);
        assert_eq!(owners[11359], 3199);
    }

    #[test]
    fn test_new_mesh() {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut m = FoamMesh::new(d.join("resources/test/cavity/")).unwrap();
        match m.read_cell_centers(m.path.join("0.5/C")) {
            Err(e) => panic!("{:?}", e),
            Ok(_) => {}
        }
        assert_relative_eq!(
            m.cell_centers.unwrap()[3199],
            Point3::new(0.09875_f64, 0.09875_f64, 0.0075_f64)
        );
    }
}
