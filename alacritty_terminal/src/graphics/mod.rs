//! This module implements the logic to manage graphic items included in a
//! `Grid` instance.

pub mod osc1337;
pub mod sixel;

use std::cmp::min;
use std::mem;
use std::sync::{Arc, Weak};

use image::DynamicImage;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::term::color::Rgb;

/// Max allowed dimensions (width, height) for the graphic, in pixels.
pub const MAX_GRAPHIC_DIMENSIONS: (usize, usize) = (4096, 4096);

/// Unique identifier for every graphic added to a grid.
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, Copy, Hash, PartialOrd, Ord)]
pub struct GraphicId(u64);

/// Reference to a texture stored in the display.
///
/// When all references to a single texture are removed, its identifier is
/// added to the remove queue.
#[derive(Clone, Debug)]
pub struct TextureRef {
    /// Graphic identifier.
    pub id: GraphicId,

    /// Queue to track removed references.
    pub remove_queue: Weak<Mutex<Vec<GraphicId>>>,
}

impl PartialEq for TextureRef {
    fn eq(&self, t: &Self) -> bool {
        // Ignore remove_queue.
        self.id == t.id
    }
}

impl Eq for TextureRef {}

impl Drop for TextureRef {
    fn drop(&mut self) {
        if let Some(remove_queue) = self.remove_queue.upgrade() {
            remove_queue.lock().push(self.id);
        }
    }
}

/// Graphic data stored in a single cell.
#[derive(Eq, PartialEq, Clone, Debug)]
pub struct GraphicCell {
    /// Texture to draw the graphic in this cell.
    pub texture: Arc<TextureRef>,

    /// Offset in the x direction.
    pub offset_x: u16,

    /// Offset in the y direction.
    pub offset_y: u16,
}

impl GraphicCell {
    /// Graphic identifier of the texture in this cell.
    #[inline]
    pub fn graphic_id(&self) -> GraphicId {
        self.texture.id
    }
}

/// Specifies the format of the pixel data.
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, Copy)]
pub enum ColorType {
    /// 3 bytes per pixel (red, green, blue).
    Rgb,

    /// 4 bytes per pixel (red, green, blue, alpha).
    Rgba,
}

/// Unit to specify a dimension to resize the graphic.
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Copy, Debug)]
pub enum ResizeParameter {
    /// Dimension is computed from the original graphic dimensions.
    Auto,

    /// Size is specified in number of grid cells.
    Cells(u32),

    /// Size is specified in number pixels.
    Pixels(u32),

    /// Size is specified in a percent of the window.
    WindowPercent(u32),
}

/// Dimensions to resize a graphic.
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Copy, Debug)]
pub struct ResizeCommand {
    pub width: ResizeParameter,

    pub height: ResizeParameter,

    pub preserve_aspect_ratio: bool,
}

/// Defines a single graphic read from the PTY.
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug)]
pub struct GraphicData {
    /// Graphics identifier.
    pub id: GraphicId,

    /// Width, in pixels, of the graphic.
    pub width: usize,

    /// Height, in pixels, of the graphic.
    pub height: usize,

    /// Color type of the pixels.
    pub color_type: ColorType,

    /// Pixels data.
    pub pixels: Vec<u8>,

    /// Render graphic in a different size.
    pub resize: Option<ResizeCommand>,
}

impl GraphicData {
    /// Create an instance from [`image::DynamicImage`].
    pub fn from_dynamic_image(id: GraphicId, image: DynamicImage) -> Self {
        let color_type;
        let width;
        let height;
        let pixels;

        match image {
            DynamicImage::ImageRgb8(image) => {
                color_type = ColorType::Rgb;
                width = image.width() as usize;
                height = image.height() as usize;
                pixels = image.into_raw();
            },

            DynamicImage::ImageRgba8(image) => {
                color_type = ColorType::Rgba;
                width = image.width() as usize;
                height = image.height() as usize;
                pixels = image.into_raw();
            },

            _ => {
                // Non-RGB image. Convert it to RGBA.
                let image = image.into_rgba8();
                color_type = ColorType::Rgba;
                width = image.width() as usize;
                height = image.height() as usize;
                pixels = image.into_raw();
            },
        }

        GraphicData { id, width, height, color_type, pixels, resize: None }
    }

    /// Resize the graphic according to the dimensions in the `resize` field.
    pub fn resized(
        self,
        cell_width: usize,
        cell_height: usize,
        view_width: usize,
        view_height: usize,
    ) -> Option<Self> {
        let resize = match self.resize {
            Some(resize) => resize,
            None => return Some(self),
        };

        if (resize.width == ResizeParameter::Auto && resize.height == ResizeParameter::Auto)
            || self.height == 0
            || self.width == 0
        {
            return Some(self);
        }

        let mut width = match resize.width {
            ResizeParameter::Auto => 1,
            ResizeParameter::Pixels(n) => n as usize,
            ResizeParameter::Cells(n) => n as usize * cell_width,
            ResizeParameter::WindowPercent(n) => n as usize * view_width / 100,
        };

        let mut height = match resize.height {
            ResizeParameter::Auto => 1,
            ResizeParameter::Pixels(n) => n as usize,
            ResizeParameter::Cells(n) => n as usize * cell_height,
            ResizeParameter::WindowPercent(n) => n as usize * view_height / 100,
        };

        if width == 0 || height == 0 {
            return None;
        }

        // Compute "auto" dimensions.
        if resize.width == ResizeParameter::Auto {
            width = self.width * height / self.height;
        }

        if resize.height == ResizeParameter::Auto {
            height = self.height * width / self.width;
        }

        // Limit size to MAX_GRAPHIC_DIMENSIONS.
        width = min(width, MAX_GRAPHIC_DIMENSIONS.0);
        height = min(height, MAX_GRAPHIC_DIMENSIONS.1);

        log::trace!("Resize new graphic to width={}, height={}", width, height,);

        // Create a new DynamicImage to resize the graphic.
        let dynimage = match self.color_type {
            ColorType::Rgb => {
                let buffer =
                    image::RgbImage::from_raw(self.width as u32, self.height as u32, self.pixels)?;
                DynamicImage::ImageRgb8(buffer)
            },

            ColorType::Rgba => {
                let buffer =
                    image::RgbaImage::from_raw(self.width as u32, self.height as u32, self.pixels)?;
                DynamicImage::ImageRgba8(buffer)
            },
        };

        // Finally, use `resize` or `resize_exact` to make the new image.
        let width = width as u32;
        let height = height as u32;
        let filter = image::imageops::FilterType::Triangle;

        let new_image = if resize.preserve_aspect_ratio {
            dynimage.resize(width, height, filter)
        } else {
            dynimage.resize_exact(width, height, filter)
        };

        Some(Self::from_dynamic_image(self.id, new_image))
    }
}

/// Queues to add or to remove the textures in the display.
pub struct UpdateQueues {
    /// Graphics read from the PTY.
    pub pending: Vec<GraphicData>,

    /// Graphics removed from the grid.
    pub remove_queue: Vec<GraphicId>,
}

/// Track changes in the grid to add or to remove graphics.
#[derive(Clone, Debug, Default)]
pub struct Graphics {
    /// Last generated identifier.
    pub last_id: u64,

    /// New graphics, received from the PTY.
    pub pending: Vec<GraphicData>,

    /// Graphics removed from the grid.
    pub remove_queue: Arc<Mutex<Vec<GraphicId>>>,

    /// Shared palette for Sixel graphics.
    pub sixel_shared_palette: Option<Vec<Rgb>>,
}

impl Graphics {
    /// Generate a new graphic identifier.
    pub fn next_id(&mut self) -> GraphicId {
        self.last_id += 1;
        GraphicId(self.last_id)
    }

    /// Get queues to update graphics in the grid.
    ///
    /// If all queues are empty, it returns `None`.
    pub fn take_queues(&mut self) -> Option<UpdateQueues> {
        let mut remove_queue = self.remove_queue.lock();
        if remove_queue.is_empty() && self.pending.is_empty() {
            return None;
        }

        let remove_queue = mem::take(&mut *remove_queue);

        Some(UpdateQueues { pending: mem::take(&mut self.pending), remove_queue })
    }
}
