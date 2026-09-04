//! Piece-set rendering.
//!
//! Piece sets are directories of SVGs named as Lichess names them (`wK.svg`,
//! `bN.svg`, and so on), installed under the data directory rather than shipped
//! with the application — piece art carries its own licensing, which is not
//! necessarily compatible with this project's.
//!
//! The SVGs are rasterised with `resvg` rather than through GTK: current
//! librsvg no longer installs a gdk-pixbuf loader, so GTK cannot decode SVG on
//! its own.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use gtk4::gdk;
use gtk4::glib::Bytes;
use gtk4::prelude::*;
use shakmaty::{Color, Role};

/// Edge length the pieces are rasterised at. Squares are smaller than this in
/// any realistic window, so they are always scaled down, never up.
const RENDER_PX: u32 = 160;

/// Governor — like most piece sets — draws its dark side as a neutral warm grey
/// (`#514b46`) with near-white specular highlights. Scaling all channels by a
/// constant darkens the highlights and the outlines along with the body, which
/// flattens the shading and is exactly what makes the result look moulded
/// rather than carved. Instead the midtones are warmed toward walnut while the
/// outlines are left black and the gloss is compressed.
///
/// Wood ramp, as per-channel gains applied to a pixel's luminance.
///
/// The channels are kept close together on purpose. Pushing red above one and
/// blue far below it produces a saturated red-brown that reads as painted
/// rather than as timber; real dark wood is only mildly warmer than neutral.
const WOOD: [f32; 3] = [0.98, 0.78, 0.58];
/// Below this luminance a pixel is outline, and is left alone.
const OUTLINE_BELOW: f32 = 0.03;
/// Luminance at which tinting reaches full strength.
const OUTLINE_FULL: f32 = 0.24;
/// Highlights above this luminance are compressed toward satin.
const GLOSS_KNEE: f32 = 0.70;
/// How much of a highlight's excess brightness survives.
const GLOSS_KEEP: f32 = 0.42;
/// Overall blend toward the wood ramp.
const WOOD_STRENGTH: f32 = 0.94;

const FILES: [(Color, Role, &str); 12] = [
    (Color::White, Role::King, "wK"),
    (Color::White, Role::Queen, "wQ"),
    (Color::White, Role::Rook, "wR"),
    (Color::White, Role::Bishop, "wB"),
    (Color::White, Role::Knight, "wN"),
    (Color::White, Role::Pawn, "wP"),
    (Color::Black, Role::King, "bK"),
    (Color::Black, Role::Queen, "bQ"),
    (Color::Black, Role::Rook, "bR"),
    (Color::Black, Role::Bishop, "bB"),
    (Color::Black, Role::Knight, "bN"),
    (Color::Black, Role::Pawn, "bP"),
];

pub struct PieceSet {
    name: String,
    textures: HashMap<(Color, Role), gdk::Texture>,
}

impl PieceSet {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn texture(&self, color: Color, role: Role) -> Option<&gdk::Texture> {
        self.textures.get(&(color, role))
    }

    /// Load every piece from a directory. A set missing any piece is rejected
    /// outright — a board with holes in it is worse than the glyph fallback.
    pub fn load(dir: &Path) -> Option<Self> {
        let mut textures = HashMap::with_capacity(FILES.len());
        for (color, role, stem) in FILES {
            // The light side already ships in a cream that reads as wood.
            let woodify = color == Color::Black;
            let texture = rasterise(&dir.join(format!("{stem}.svg")), woodify)?;
            textures.insert((color, role), texture);
        }
        Some(Self {
            name: dir.file_name()?.to_string_lossy().into_owned(),
            textures,
        })
    }

    /// Find a usable set: the one named by `OMACHESS_PIECE_SET` if set,
    /// otherwise any installed set, in directory order.
    pub fn discover(root: &Path) -> Option<Self> {
        if let Some(name) = std::env::var_os("OMACHESS_PIECE_SET") {
            return Self::load(&root.join(name));
        }
        let mut candidates: Vec<PathBuf> = fs::read_dir(root)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        candidates.sort();
        candidates.iter().find_map(|dir| Self::load(dir))
    }
}

fn rasterise(path: &Path, woodify: bool) -> Option<gdk::Texture> {
    let data = fs::read(path).ok()?;
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;

    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return None;
    }
    // Fit the drawing into a square without distorting it.
    let scale = RENDER_PX as f32 / size.width().max(size.height());
    let offset_x = (RENDER_PX as f32 - size.width() * scale) / 2.0;
    let offset_y = (RENDER_PX as f32 - size.height() * scale) / 2.0;

    let mut pixmap = resvg::tiny_skia::Pixmap::new(RENDER_PX, RENDER_PX)?;
    let transform =
        resvg::tiny_skia::Transform::from_translate(offset_x, offset_y).pre_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    if woodify {
        wood_tint(pixmap.data_mut());
    }

    Some(
        gdk::MemoryTexture::new(
            RENDER_PX as i32,
            RENDER_PX as i32,
            gdk::MemoryFormat::R8g8b8a8Premultiplied,
            &Bytes::from(pixmap.data()),
            RENDER_PX as usize * 4,
        )
        .upcast(),
    )
}

/// Recolour a greyscale piece as dark walnut, preserving its shading.
///
/// The buffer is premultiplied RGBA, so each pixel is divided by its alpha
/// before the colour maths and multiplied back afterwards; operating on
/// premultiplied values directly would tint the anti-aliased edges differently
/// from the solid interior and leave a halo.
fn wood_tint(data: &mut [u8]) {
    for pixel in data.as_chunks_mut::<4>().0 {
        let alpha = f32::from(pixel[3]) / 255.0;
        if alpha <= 0.0 {
            continue;
        }

        let straight = [
            f32::from(pixel[0]) / 255.0 / alpha,
            f32::from(pixel[1]) / 255.0 / alpha,
            f32::from(pixel[2]) / 255.0 / alpha,
        ];
        let luma = 0.2126 * straight[0] + 0.7152 * straight[1] + 0.0722 * straight[2];

        // Outlines stay black; the tint fades in above them.
        let weight = smoothstep(OUTLINE_BELOW, OUTLINE_FULL, luma) * WOOD_STRENGTH;

        // Pull the specular highlights down so the piece reads as satin wood
        // rather than polished plastic.
        let level = if luma > GLOSS_KNEE {
            GLOSS_KNEE + (luma - GLOSS_KNEE) * GLOSS_KEEP
        } else {
            luma
        };

        for channel in 0..3 {
            let target = (level * WOOD[channel]).clamp(0.0, 1.0);
            let blended = straight[channel] * (1.0 - weight) + target * weight;
            pixel[channel] = (blended.clamp(0.0, 1.0) * alpha * 255.0).round() as u8;
        }
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
