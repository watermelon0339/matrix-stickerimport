#[cfg(feature = "ffmpeg")]
use crate::video::webm2webp;
use crate::{
	database,
	error::{Error, NoMimeType},
	matrix::{self, Config, Mxc}
};
#[cfg(feature = "lottie")]
use lottieconv::{Animation, Converter, Rgba};
use once_cell::sync::Lazy;
use serde::Deserialize;
#[cfg(any(feature = "ffmpeg", feature = "lottie"))]
use std::io::Write;
use std::{io::Read, path::Path, sync::Arc};
use strum_macros::Display;
#[cfg(feature = "lottie")]
use tempfile::NamedTempFile;

use photon_rs::transform;
use photon_rs::native::{open_image_from_bytes, image_to_bytes};

#[cfg(feature = "log")]
use log::{info, warn};

// todo: remove copy trait. Or will gif support droppet first?
#[derive(Clone, Copy, Debug, Default, Deserialize, Display)]
#[serde(tag = "animation_format", rename_all = "lowercase")]
pub enum AnimationFormat {
	#[cfg(feature = "lottie")]
	Gif { transparent_color: Rgba },

	#[default]
	Webp
}

#[derive(Clone)]
/// Generic image struct, containing the image data and its meta data.
pub struct Image {
	pub file_name: String,
	pub data: Arc<Vec<u8>>,
	pub width: u32,
	pub height: u32
}

fn rayon_run<F, T>(callback: F) -> T
where
	F: FnOnce() -> T + Send,
	T: Send,
	for<'a> &'a mut T: Send
{
	let mut result: Option<T> = None;
	rayon::scope(|s| {
		s.spawn(|_| result = Some(callback()));
	});
	result.unwrap()
}

impl Image {
	pub fn mime_type(&self) -> Result<String, NoMimeType> {
		let extension = Path::new(&self.file_name)
			.extension()
			.ok_or_else(|| NoMimeType)?
			.to_str()
			.unwrap(); //this must be valid utf8 since we use a string as input
		Ok(if extension == "webm" {
			format!("video/{extension}",)
		} else {
			format!("image/{extension}",)
		})
	}

	/// unpack gzip compression `tgs`, converting it to `lottie`, ignore other formats
	pub async fn unpack_tgs(mut self) -> Result<Self, Error> {
		if !self.file_name.ends_with(".tgs") {
			return Ok(self);
		}
		let image: Result<Image, Error> = tokio::task::spawn_blocking(move || {
			rayon_run(move || {
				let mut output = Vec::new();
				let input_reader = &**self.data;
				flate2::read::GzDecoder::new(input_reader).read_to_end(&mut output)?;
				self.data = Arc::new(output);
				self.file_name.truncate(self.file_name.len() - 3);
				self.file_name += "lottie";
				Ok(self)
			})
		})
		.await?;
		Ok(image?)
	}

	/// convert `tgs` image to webp or gif, ignore other formats
	#[cfg(feature = "lottie")]
	pub async fn convert_lottie(self, animation_format: AnimationFormat, max_width: Option<u32>, max_height: Option<u32>) -> Result<Self, Error> {
		use lottieconv::Size;

		if !self.file_name.ends_with(".lottie") {
			return Ok(self);
		}
		let mut image = self.unpack_tgs().await?;
		tokio::task::spawn_blocking(move || {
			rayon_run(move || {
				//save to image to file
				let mut tmp = NamedTempFile::new()?;
				tmp.write_all(&image.data)?;
				tmp.flush()?;
				let animation = Animation::from_file(tmp.path()).ok_or_else(|| Error::AnimationLoadError)?;
				let size = animation.size();
				let aspect_ratio = size.width.clone() as f32 / size.height.clone() as f32;
				let (new_width, new_height) = Self::resize_preserving_aspect_ratio(size.width as u32, size.height as u32, max_width, max_height);
				let new_size = Size {
					width: new_width as usize,
					height: new_height as usize
				};
				image.file_name.truncate(image.file_name.len() - 6);
				let converter = Converter::new(animation);
				match animation_format {
					AnimationFormat::Gif { transparent_color } => {
						let mut data = Vec::new();
						converter.with_size(new_size).gif(transparent_color, &mut data)?.convert()?;
						image.data = Arc::new(data);
						image.file_name += "gif";
					},
					AnimationFormat::Webp => {
						image.data = Arc::new(converter.with_size(new_size).webp()?.convert()?.to_vec());
						image.file_name += "webp";
					}
				}
				image.width = new_size.width as u32;
				image.height = new_size.height as u32;
				Ok(image)
			})
		})
		.await?
	}

	#[cfg(feature = "ffmpeg")]
	/// convert `webm` video stickers to webp, ignore other formats
	pub async fn convert_webm2webp(mut self, new_width: Option<u32>, new_height: Option<u32>) -> Result<Self, Error> {
		if !self.file_name.ends_with(".webm") {
			return Ok(self);
		}

		tokio::task::spawn_blocking(move || {
			rayon_run(move || {
				let mut tmp = tempfile::Builder::new().suffix(".webm").tempfile()?;
				tmp.write_all(&self.data)?;
				tmp.flush()?;

				self.file_name.truncate(self.file_name.len() - 1);
				self.file_name += "p";
				let (webp, width, height) = webm2webp(&tmp.path(), new_width, new_height)?;
				self.data = Arc::new(webp.to_vec());
				self.width = width;
				self.height = height;

				Ok(self)
			})
		})
		.await?
	}

	///upload image to matrix
	/// return mxc_url and true if image was uploaded now; false if it was already uploaded before and exist at the database
	pub async fn upload<D>(&self, matrix_config: &Config, database: Option<&D>) -> Result<(Mxc, bool), Error>
	where
		D: database::Database
	{
		let hash = Lazy::new(|| database::hash(&self.data));

		// if database is some and datbase.unwrap().get() is also some
		if let Some(db) = database {
			if let Some(url) = db.get(&hash).await.map_err(Error::Database)? {
				return Ok((url.into(), false));
			}
		}

		let mxc = matrix::upload(matrix_config, &self.file_name, self.data.clone(), &self.mime_type()?).await?;
		if let Some(db) = database {
			db.add(*hash, mxc.url().to_owned()).await.map_err(Error::Database)?;
		}
		Ok((mxc, true))
	}

	fn resize_preserving_aspect_ratio(
		width: u32,
		height: u32,
		max_width: Option<u32>,
		max_height: Option<u32>
	) -> (u32, u32) {
		let aspect_ratio = width as f64 / height as f64;
	
		match (max_width, max_height) {
			(None, None) => (width, height),
			(Some(w), None) => {
				let new_width = w as f64;
				let new_height = new_width / aspect_ratio;
				return (new_width.round() as u32, new_height.round() as u32);
			},
			(None, Some(h)) => {
				let new_height = h as f64;
				let new_width = new_height * aspect_ratio;
				return (new_width.round() as u32, new_height.round() as u32);
			},
			(Some(w), Some(h)) => {
				let max_w = w as f64;
				let max_h = h as f64;
		
				let scale_w = max_w / width as f64;
				let scale_h = max_h / height as f64;
				let scale = scale_w.min(scale_h);
		
				let new_width = (width as f64 * scale).round();
				let new_height = (height as f64 * scale).round();
		
				return (new_width as u32, new_height as u32);
			}
		}
	}

	pub fn resize(mut self, max_width: u32, max_height: u32) -> Result<Self, Error> {
		let mut img = open_image_from_bytes(&self.data).unwrap();
		let img_width = img.clone().get_width();
		let img_height = img.clone().get_height();
		let (width, height) = Self::resize_preserving_aspect_ratio(img_width, img_height, Some(max_width), Some(max_height));
		img = transform::resize(&mut img, width, height, transform::SamplingFilter::Lanczos3);
		self.data = Arc::new(img.get_bytes_webp().to_vec());
		self.width = width;
		self.height = height;
		return Ok(self);
	}
}
