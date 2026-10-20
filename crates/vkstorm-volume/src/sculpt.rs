//! The sculpt mesh resolution rule of `LLVolume::sculpt`
//! (`sculpt_calc_mesh_resolution`): how many path and profile points a
//! sculpt map gets at a LOD. The map-to-mesh step itself is the next rung;
//! this is here because the profile and path generation already take the
//! sizes it produces.

const SCULPT_REZ_1: i32 = 6; // 6 looks round whereas 4 looks square
const SCULPT_REZ_2: i32 = 8;
const SCULPT_REZ_3: i32 = 16;
const SCULPT_REZ_4: i32 = 32;

/// `sculpt_sides`: detail is usually one of 1, 1.5, 2.5, 4.
pub fn sculpt_sides(detail: f32) -> i32 {
    if detail <= 1.0 {
        SCULPT_REZ_1
    } else if detail <= 2.0 {
        SCULPT_REZ_2
    } else if detail <= 3.0 {
        SCULPT_REZ_3
    } else {
        SCULPT_REZ_4
    }
}

/// `sculpt_calc_mesh_resolution`: `(s, t)`, the requested path and profile
/// sizes. The aspect ratio of the mesh is as close as possible to the map's
/// while using all vertices allowed by the LOD and the map.
pub fn mesh_resolution(width: u16, height: u16, detail: f32) -> (i32, i32) {
    let max_vertices_lod = (sculpt_sides(detail) as f64).powf(2.0) as i32;
    let max_vertices_map = width as i32 * height as i32 / 4;

    let vertices = if max_vertices_map > 0 {
        max_vertices_lod.min(max_vertices_map)
    } else {
        max_vertices_lod
    };

    let ratio = if width == 0 || height == 0 {
        1.0f32
    } else {
        width as f32 / height as f32
    };

    let mut s = ((vertices as f32 / ratio).sqrt()) as i32;
    s = s.max(4); // no degenerate sizes, please
    let mut t = vertices / s;
    t = t.max(4);
    s = vertices / t;
    (s, t)
}
