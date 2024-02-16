//! Image processing functions for generating combined thumbnails.

use std::io::Cursor;

use image::{
    imageops,
    imageops::FilterType,
    DynamicImage,
    GenericImageView,
    ImageError,
    ImageOutputFormat,
};
use log::debug;
use rayon::prelude::*;
use thiserror::Error;

/// Errors that can occur during image processing.
#[derive(Debug, Error)]
pub enum ProcessingError {
    #[error("Image array is empty")]
    EmptyImageArray,
    #[error("Image array has too many images, maximum is 4")]
    TooManyImages,
    #[error("Could not find image with most pixels, array is likely empty")]
    CouldNotFindMostPixels,
    #[error("Image encoding error: {0}")]
    ImageError(#[from] ImageError),
}

/// A basic wrapper struct to hold a combined thumbnail's bytes for passing back from an axum
/// handler.
pub struct CombinedThumbnail {
    inner: Vec<u8>,
}

impl CombinedThumbnail {
    pub fn new(image: DynamicImage, format: ImageOutputFormat) -> Result<Self, ImageError> {
        let mut buffer = Cursor::new(Vec::new());
        image.write_to(&mut buffer, format)?;

        Ok(CombinedThumbnail {
            inner: buffer.into_inner(),
        })
    }

    pub fn to_bytes(&self) -> &[u8] {
        &self.inner
    }
}

/// Generate a combined thumbnail from a list of images, adding a nice blur effect as a background.
pub fn generate_combined_thumbnail(
    images: Vec<DynamicImage>,
) -> Result<CombinedThumbnail, ProcessingError> {
    let total_size = get_total_img_size(&images)?;
    let combined = combine_images(&images, total_size.0, total_size.1, true)?;
    let mut background = combine_images(&images, total_size.0, total_size.1, false)?.blur(50.0);
    imageops::overlay(&mut background, &combined, 0, 0);

    let thumbnail = CombinedThumbnail::new(background, ImageOutputFormat::Png)?;
    Ok(thumbnail)
}

/// Combine two images in a horizontal layout, with an optional vertical offset.
fn layout_horizontal(new_image: &mut DynamicImage, images: &[DynamicImage], y_offset: u32) {
    let mut x_offset: u32 = 0;
    for image in images {
        imageops::overlay(new_image, image, x_offset as i64, y_offset as i64);
        debug!("Overlaying image at x: {x_offset}, y: {y_offset}");
        x_offset += image.width();
    }
}

/// Takes a slice of images and combines them into a single image, appropriately laid out based on
/// the number of images and their sizes.
fn combine_images(
    images: &[DynamicImage],
    total_width: u32,
    total_height: u32,
    pad: bool,
) -> Result<DynamicImage, ProcessingError> {
    // If there is only one image, return it
    if images.len() == 1 {
        return Ok(images[0].to_owned());
    }

    let mut new_image = DynamicImage::new_rgba8(total_width, total_height);
    let top_img = find_img_with_most_pixels(images)?;

    let mut scaled_images =
        scale_all_images_to_same_size(images, top_img.width(), top_img.height(), pad);

    match scaled_images.len() {
        0 => return Err(ProcessingError::EmptyImageArray),
        1 => return Ok(images[0].to_owned()),
        2 => {
            // If there are two images, combine them horizontally
            layout_horizontal(&mut new_image, &scaled_images, 0);
        }
        3 => {
            // If there are three images, combine the first two horizontally, then combine the last
            // one vertically
            layout_horizontal(&mut new_image, &scaled_images[..2], 0);

            // Take the last image, treat it like an image array and scale it to the total width,
            // but with the same height as all individual images
            let processed_last_img = scale_all_images_to_same_size(
                &[scaled_images[2].to_owned()],
                total_width,
                top_img.height(),
                pad,
            );
            scaled_images[2] = processed_last_img.first().unwrap().to_owned();

            // Overlay the third image below the first two with a vertical offset
            layout_horizontal(
                &mut new_image,
                &scaled_images[2..],
                scaled_images[0].height(),
            );
        }
        4 => {
            // If there are four images, combine the images in a 2x2 grid with the first two on top
            // and the last two on the bottom
            layout_horizontal(&mut new_image, &scaled_images[..2], 0);
            layout_horizontal(
                &mut new_image,
                &scaled_images[2..],
                scaled_images[0].height(),
            );
        }
        _ => return Err(ProcessingError::TooManyImages),
    }

    Ok(new_image)
}

/// Find the image with biggest resolution in an array of images.
fn find_img_with_most_pixels(images: &[DynamicImage]) -> Result<&DynamicImage, ProcessingError> {
    images
        .par_iter()
        .max_by_key(|img| img.dimensions().0 * img.dimensions().1)
        .ok_or(ProcessingError::CouldNotFindMostPixels)
}

/// Get the total size of the combined image, based on the number of images.
fn get_total_img_size(images: &[DynamicImage]) -> Result<(u32, u32), ProcessingError> {
    let max_image = find_img_with_most_pixels(images)?;
    let (width, height) = max_image.dimensions();
    let size = match images.len() {
        1 => (width, height),
        2 => (width * 2, height),
        _ => (width * 2, height * 2),
    };
    Ok(size)
}

/// Scale an image to a target width and height, with an optional padding to fill the target size in
/// an aesthetically pleasing way.
fn scale_image_iterable(
    image: &DynamicImage,
    target_width: u32,
    target_height: u32,
    pad: bool,
) -> DynamicImage {
    if pad {
        let (width, height) = image.dimensions();

        // Calculate the new size while maintaining the aspect ratio
        let aspect_ratio = width as f64 / height as f64;
        let (new_width, new_height) = if aspect_ratio > (target_width as f64 / target_height as f64)
        {
            (target_width, (target_width as f64 / aspect_ratio) as u32)
        } else {
            ((target_height as f64 * aspect_ratio) as u32, target_height)
        };

        let resized = image.resize_exact(new_width, new_height, FilterType::Lanczos3);

        // Calculate the positions to place the resized image
        let x = (target_width - new_width) / 2;
        let y = (target_height - new_height) / 2;

        // Paste the resized image onto a new image with the new positions
        let mut new_img = DynamicImage::new_rgba8(target_width, target_height);
        imageops::overlay(&mut new_img, &resized, x as i64, y as i64);

        new_img
    } else {
        image.resize_exact(target_width, target_height, FilterType::Gaussian)
    }
}

/// Takes a slice of images and scales them all to the same size, with an optional padding to fill
/// the target size in an aesthetically pleasing way.
fn scale_all_images_to_same_size(
    image_array: &[DynamicImage],
    target_width: u32,
    target_height: u32,
    pad: bool,
) -> Vec<DynamicImage> {
    image_array
        .par_iter()
        .map(|image| scale_image_iterable(image, target_width, target_height, pad))
        .collect()
}
