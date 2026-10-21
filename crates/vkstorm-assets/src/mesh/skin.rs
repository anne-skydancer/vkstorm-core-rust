//! `LLMeshSkinInfo::fromLLSD` and `LLModel::Decomposition::fromLLSD`: the
//! skin and physics blocks of a mesh asset.

use vkstorm_llsd::Value;

use super::inflate::{unzip_llsd, ZipResult};
use super::llsd::{as_binary, as_boolean, as_real, as_string, get, has, index, size, vector3};

/// A 4x4 matrix as `LLMatrix4a` holds it: four rows of four floats.
pub type Matrix = [[f32; 4]; 4];

pub const IDENTITY: Matrix = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

/// `LLMeshSkinInfo`'s parsed content.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkinInfo {
    pub joint_names: Vec<Vec<u8>>,
    /// `mJointNums`: -1 per joint name until the avatar maps them.
    pub joint_nums: Vec<i32>,
    pub inv_bind_matrix: Vec<Matrix>,
    pub alternate_bind_matrix: Vec<Matrix>,
    pub bind_shape_matrix: Matrix,
    /// `mBindPoseMatrix[i] = mBindShapeMatrix * mInvBindMatrix[i]`.
    pub bind_pose_matrix: Vec<Matrix>,
    pub pelvis_offset: f32,
    pub lock_scale_if_joint_position: bool,
}

/// `rowMul(row, mat)`: the SSE row-by-matrix product, lane for lane.
fn row_mul(row: &[f32; 4], m: &Matrix) -> [f32; 4] {
    let mut r = [0.0f32; 4];
    for k in 0..4 {
        r[k] = row[0] * m[0][k];
    }
    for i in 1..4 {
        for k in 0..4 {
            r[k] += row[i] * m[i][k];
        }
    }
    r
}

/// `matMul(a, b, res)`.
pub fn mat_mul(a: &Matrix, b: &Matrix) -> Matrix {
    [row_mul(&a[0], b), row_mul(&a[1], b), row_mul(&a[2], b), row_mul(&a[3], b)]
}

fn matrix_from(v: &Value) -> Matrix {
    let mut m = [[0.0f32; 4]; 4];
    for (j, row) in m.iter_mut().enumerate() {
        for (k, cell) in row.iter_mut().enumerate() {
            *cell = as_real(index(v, j * 4 + k)) as f32;
        }
    }
    m
}

impl SkinInfo {
    /// `fromLLSD` on a parsed skin block. Starts from the default state
    /// (identity bind shape, zero pelvis offset).
    pub fn from_llsd(skin: &Value) -> SkinInfo {
        let mut s = SkinInfo { bind_shape_matrix: IDENTITY, ..Default::default() };
        if has(skin, "joint_names") {
            let names = get(skin, "joint_names");
            for i in 0..size(names) {
                s.joint_names.push(as_string(index(names, i)));
                s.joint_nums.push(-1);
            }
        }
        if has(skin, "inverse_bind_matrix") {
            let mats = get(skin, "inverse_bind_matrix");
            for i in 0..size(mats) {
                s.inv_bind_matrix.push(matrix_from(index(mats, i)));
            }
            if s.joint_names.len() != s.inv_bind_matrix.len() {
                crate::ll_warns!("Joints vs bind matrix count mismatch. Dropping joint bindings.");
                s.joint_names.clear();
                s.joint_nums.clear();
                s.inv_bind_matrix.clear();
            }
        }
        if has(skin, "bind_shape_matrix") {
            s.bind_shape_matrix = matrix_from(get(skin, "bind_shape_matrix"));
        }
        if has(skin, "alt_inverse_bind_matrix") {
            let mats = get(skin, "alt_inverse_bind_matrix");
            for i in 0..size(mats) {
                s.alternate_bind_matrix.push(matrix_from(index(mats, i)));
            }
        }
        if has(skin, "pelvis_offset") {
            s.pelvis_offset = as_real(get(skin, "pelvis_offset")) as f32;
        }
        s.lock_scale_if_joint_position = if has(skin, "lock_scale_if_joint_position") {
            as_boolean(get(skin, "lock_scale_if_joint_position"))
        } else {
            false
        };
        s.bind_pose_matrix = s.inv_bind_matrix.iter().map(|m| mat_mul(&s.bind_shape_matrix, m)).collect();
        s
    }

    /// `skinInfoReceived`'s parse of the zipped block: an empty block
    /// yields the default skin info (the C++ parsed nothing and built from
    /// an undefined LLSD).
    pub fn from_zipped(data: &[u8]) -> Result<SkinInfo, ZipResult> {
        if data.is_empty() {
            return Ok(SkinInfo::from_llsd(&Value::Undefined));
        }
        let v = unzip_llsd(data)?;
        Ok(SkinInfo::from_llsd(&v))
    }
}

/// `LLModel::Decomposition`'s parsed hulls.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decomposition {
    /// `mHull`: the convex hulls' points.
    pub hulls: Vec<Vec<[f32; 3]>>,
    /// `mBaseHull`.
    pub base_hull: Vec<[f32; 3]>,
    /// Whether the block had no `BoundingVerts` (the C++ cleared
    /// `mBaseHullMesh` to mark the decomposition loaded without a base hull).
    pub base_hull_mesh_cleared: bool,
}

/// A hull point: `(F32)p/65535.f*range+min` per component.
#[inline]
fn unquantize(p: u16, range: f32, min: f32) -> f32 {
    p as f32 / 65535.0 * range + min
}

impl Decomposition {
    /// `fromLLSD`. `Err` when a hull list runs past its positions (the C++
    /// read past the buffer).
    pub fn from_llsd(decomp: &Value) -> Result<Decomposition, &'static str> {
        let mut d = Decomposition::default();
        if has(decomp, "HullList") && has(decomp, "Positions") {
            let hulls = as_binary(get(decomp, "HullList"));
            let position = as_binary(get(decomp, "Positions"));
            let (min, max) = if has(decomp, "Min") {
                (vector3(get(decomp, "Min")), vector3(get(decomp, "Max")))
            } else {
                ([-0.5, -0.5, -0.5], [0.5, 0.5, 0.5])
            };
            let range = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
            let mut p = 0usize;
            for &count_byte in hulls.iter() {
                let count: usize = if count_byte == 0 { 256 } else { count_byte as usize };
                let mut hull = Vec::with_capacity(count);
                for _ in 0..count {
                    if position.len() < (p + 3) * 2 {
                        return Err("HullList runs past Positions");
                    }
                    let q = [
                        u16::from_le_bytes([position[2 * p], position[2 * p + 1]]),
                        u16::from_le_bytes([position[2 * (p + 1)], position[2 * (p + 1) + 1]]),
                        u16::from_le_bytes([position[2 * (p + 2)], position[2 * (p + 2) + 1]]),
                    ];
                    hull.push([unquantize(q[0], range[0], min[0]), unquantize(q[1], range[1], min[1]), unquantize(q[2], range[2], min[2])]);
                    p += 3;
                }
                d.hulls.push(hull);
            }
        }
        if has(decomp, "BoundingVerts") {
            let position = as_binary(get(decomp, "BoundingVerts"));
            let (min, max) = if has(decomp, "Min") {
                (vector3(get(decomp, "Min")), vector3(get(decomp, "Max")))
            } else {
                ([-0.5, -0.5, -0.5], [0.5, 0.5, 0.5])
            };
            let range = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
            let count = (position.len() / 6) as u16 as usize;
            for j in 0..count {
                let p = j * 3;
                let q = [
                    u16::from_le_bytes([position[2 * p], position[2 * p + 1]]),
                    u16::from_le_bytes([position[2 * (p + 1)], position[2 * (p + 1) + 1]]),
                    u16::from_le_bytes([position[2 * (p + 2)], position[2 * (p + 2) + 1]]),
                ];
                d.base_hull.push([unquantize(q[0], range[0], min[0]), unquantize(q[1], range[1], min[1]), unquantize(q[2], range[2], min[2])]);
            }
        } else {
            d.base_hull_mesh_cleared = true;
        }
        Ok(d)
    }

    /// `decompositionReceived`'s parse of the zipped block.
    pub fn from_zipped(data: &[u8]) -> Result<Decomposition, ZipResult> {
        if data.is_empty() {
            return Decomposition::from_llsd(&Value::Undefined).map_err(|_| ZipResult::DataError);
        }
        let v = unzip_llsd(data)?;
        Decomposition::from_llsd(&v).map_err(|_| ZipResult::DataError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn mat_value(m: &Matrix) -> Value {
        Value::Array(m.iter().flatten().map(|x| Value::Real(*x as f64)).collect())
    }

    #[test]
    fn skin_parse_and_bind_pose() {
        let mut m = BTreeMap::new();
        m.insert(b"joint_names".to_vec(), Value::Array(vec![Value::String(b"mPelvis".to_vec()), Value::String(b"mSpine".to_vec())]));
        let a: Matrix = [[1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0], [9.0, 10.0, 11.0, 12.0], [13.0, 14.0, 15.0, 16.0]];
        m.insert(b"inverse_bind_matrix".to_vec(), Value::Array(vec![mat_value(&IDENTITY), mat_value(&a)]));
        let bs: Matrix = [[2.0, 0.0, 0.0, 0.0], [0.0, 2.0, 0.0, 0.0], [0.0, 0.0, 2.0, 0.0], [1.0, 1.0, 1.0, 1.0]];
        m.insert(b"bind_shape_matrix".to_vec(), mat_value(&bs));
        m.insert(b"pelvis_offset".to_vec(), Value::Real(0.25));
        let s = SkinInfo::from_llsd(&Value::Map(m));
        assert_eq!(s.joint_names, vec![b"mPelvis".to_vec(), b"mSpine".to_vec()]);
        assert_eq!(s.joint_nums, vec![-1, -1]);
        assert_eq!(s.bind_pose_matrix[0], bs);
        assert_eq!(s.bind_pose_matrix[1], mat_mul(&bs, &a));
        assert_eq!(s.pelvis_offset, 0.25);
        assert!(!s.lock_scale_if_joint_position);
        // Count mismatch drops the bindings.
        let mut m = BTreeMap::new();
        m.insert(b"joint_names".to_vec(), Value::Array(vec![Value::String(b"mPelvis".to_vec())]));
        m.insert(b"inverse_bind_matrix".to_vec(), Value::Array(vec![mat_value(&IDENTITY), mat_value(&a)]));
        let s = SkinInfo::from_llsd(&Value::Map(m));
        assert!(s.joint_names.is_empty() && s.inv_bind_matrix.is_empty() && s.bind_pose_matrix.is_empty());
        let empty = SkinInfo::from_zipped(&[]).unwrap();
        assert_eq!(empty.bind_shape_matrix, IDENTITY);
    }

    #[test]
    fn decomposition_parse() {
        let mut m = BTreeMap::new();
        m.insert(b"HullList".to_vec(), Value::Binary(vec![2, 1]));
        let mut pos = Vec::new();
        for q in [0u16, 65535, 32767, 65535, 0, 0, 100, 200, 300] {
            pos.extend_from_slice(&q.to_le_bytes());
        }
        m.insert(b"Positions".to_vec(), Value::Binary(pos));
        m.insert(b"Min".to_vec(), Value::Array(vec![Value::Real(-1.0), Value::Real(-1.0), Value::Real(-1.0)]));
        m.insert(b"Max".to_vec(), Value::Array(vec![Value::Real(1.0), Value::Real(1.0), Value::Real(1.0)]));
        let d = Decomposition::from_llsd(&Value::Map(m.clone())).unwrap();
        assert_eq!(d.hulls.len(), 2);
        assert_eq!(d.hulls[0].len(), 2);
        assert_eq!(d.hulls[1].len(), 1);
        assert_eq!(d.hulls[0][0], [-1.0, 1.0, 0.0f32 / 65535.0 * 2.0 - 1.0 + (32767.0f32 / 65535.0 * 2.0)]);
        assert!(d.base_hull.is_empty() && d.base_hull_mesh_cleared);
        m.insert(b"HullList".to_vec(), Value::Binary(vec![0]));
        assert!(Decomposition::from_llsd(&Value::Map(m)).is_err());
    }
}
