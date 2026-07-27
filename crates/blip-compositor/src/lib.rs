//! Real-time composition of caller-provided `CoreVideo` frames.
//!
//! This crate owns rendering only. Capture, decoding, timeline evaluation, and output delivery
//! remain with the caller. Supply the frames available for the current render and refer to them
//! by index from [`CompositorItemContent::Source`].

use anyhow::{Context as _, Result, anyhow};
use core_foundation::{
    base::{CFType, TCFType},
    boolean::CFBoolean,
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};
use core_video::{
    metal_texture::{CVMetalTexture, CVMetalTextureGetTexture},
    metal_texture_cache::CVMetalTextureCache,
    pixel_buffer::{
        CVPixelBuffer, CVPixelBufferKeys, kCVPixelFormatType_32BGRA,
        kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
    },
    pixel_buffer_pool::{CVPixelBufferPool, CVPixelBufferPoolKeys},
};
use foreign_types::ForeignType;
use metal::{MTLPixelFormat, MTLTextureUsage};
use objc2::{
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
};
use objc2_metal::{MTLTexture, MTLTextureType};
use wgpu::{
    hal::metal::{Api as MetalApi, Device as HalMetalDevice},
    util::DeviceExt as _,
};

#[allow(
    dead_code,
    unused_doc_comments,
    unused_imports,
    clippy::all,
    clippy::as_conversions,
    clippy::undocumented_unsafe_blocks,
    clippy::pedantic
)]
mod shader_bindings {
    include!(concat!(env!("OUT_DIR"), "/shader_bindings.rs"));
}

use shader_bindings::compositor as shader;

/// A Metal compositor for `CoreVideo` frames and solid-color layers.
pub struct FrameCompositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture_cache: CVMetalTextureCache,
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    dummy_texture: wgpu::Texture,
    output_pool: Option<OutputPool>,
}

struct OutputPool {
    dimensions: (usize, usize),
    pool: CVPixelBufferPool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemTransform {
    pub center: [f32; 2],
    pub size: [f32; 2],
    pub corner_radius: f32,
    pub squircle: bool,
    pub box_shadow: Option<BoxShadow>,
}

/// A drop shadow matching the offset, color, blur, and spread of CSS `box-shadow`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxShadow {
    pub offset: [f32; 2],
    pub color: [f32; 4],
    pub blur_radius: f32,
    pub spread_radius: f32,
}

impl BoxShadow {
    #[must_use]
    pub const fn new(offset: [f32; 2], color: [f32; 4]) -> Self {
        Self {
            offset,
            color,
            blur_radius: 0.0,
            spread_radius: 0.0,
        }
    }

    #[must_use]
    pub const fn with_blur_radius(mut self, blur_radius: f32) -> Self {
        self.blur_radius = blur_radius;
        self
    }

    #[must_use]
    pub const fn with_spread_radius(mut self, spread_radius: f32) -> Self {
        self.spread_radius = spread_radius;
        self
    }
}

/// A scale applied to the fully composed output around a normalized focal point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutputTransform {
    pub center: [f32; 2],
    pub scale: f32,
}

impl OutputTransform {
    pub const IDENTITY: Self = Self {
        center: [0.5, 0.5],
        scale: 1.0,
    };
}

impl ItemTransform {
    #[must_use]
    pub const fn new(center: [f32; 2], size: [f32; 2]) -> Self {
        Self {
            center,
            size,
            corner_radius: 0.0,
            squircle: false,
            box_shadow: None,
        }
    }

    #[must_use]
    pub const fn with_corner_radius(mut self, corner_radius: f32) -> Self {
        self.corner_radius = corner_radius;
        self
    }

    #[must_use]
    pub const fn with_squircle(mut self, squircle: bool) -> Self {
        self.squircle = squircle;
        self
    }

    #[must_use]
    pub const fn with_box_shadow(mut self, box_shadow: BoxShadow) -> Self {
        self.box_shadow = Some(box_shadow);
        self
    }

    #[must_use]
    pub fn clamped_corner_radius(self, canvas_size: [f32; 2]) -> f32 {
        let width = canvas_size[0] * self.size[0];
        let height = canvas_size[1] * self.size[1];
        self.corner_radius.clamp(0.0, width.min(height) * 0.5)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContentRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A frame supplied by the caller for use in a composition.
#[derive(Clone, Copy)]
pub struct CompositorSource<'a> {
    pub pixel_buffer: &'a CVPixelBuffer,
    pub content_rect: Option<ContentRect>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompositorItem {
    pub content: CompositorItemContent,
    pub transform: ItemTransform,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CompositorItemContent {
    Source(usize),
    Color([f32; 4]),
    Gradient { start: [f32; 4], end: [f32; 4] },
    Checkerboard { first: [f32; 4], second: [f32; 4] },
}

struct SourceTextures {
    frame: wgpu::Texture,
    chroma: Option<wgpu::Texture>,
    content_kind: f32,
}

impl FrameCompositor {
    /// Creates a compositor backed by a high-performance Metal device.
    ///
    /// # Errors
    ///
    /// Returns an error when a Metal adapter, device, or `CoreVideo` texture cache cannot be
    /// created.
    pub fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL,
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::default(),
            display: None,
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .context("failed to find a Metal adapter for the compositor")?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("blip-studio compositor"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        }))
        .context("failed to create the compositor GPU device")?;

        // SAFETY: The requested backend is Metal, and the cloned retain is transferred
        // to the `metal` wrapper consumed by CoreVideo's texture cache.
        let metal_device = unsafe {
            let hal_device = device
                .as_hal::<MetalApi>()
                .ok_or_else(|| anyhow!("wgpu did not create a Metal device"))?;
            metal::Device::from_ptr(Retained::into_raw(hal_device.raw_device().clone()).cast())
        };
        let texture_cache = CVMetalTextureCache::new(None, metal_device, None)
            .map_err(|status| anyhow!("failed to create CoreVideo texture cache ({status})"))?;

        let pipeline_layout = shader::create_pipeline_layout(&device);
        let shader_module = shader::create_shader_module_embed_source(&device);
        let vertex_entry = shader::vertex_entry();
        let fragment_entry = shader::fragment_entry([Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Bgra8Unorm,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        })]);
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blip compositor pipeline"),
            layout: Some(&pipeline_layout),
            vertex: shader::vertex_state(&shader_module, &vertex_entry),
            fragment: Some(shader::fragment_state(&shader_module, &fragment_entry)),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blip frame sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let dummy_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("blip solid color dummy texture"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &dummy_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255, 255, 255, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        Ok(Self {
            device,
            queue,
            texture_cache,
            pipeline,
            sampler,
            dummy_texture,
            output_pool: None,
        })
    }

    #[allow(clippy::too_many_lines)]
    /// Renders the items in slice order into a pooled BGRA pixel buffer.
    ///
    /// # Errors
    ///
    /// Returns an error when dimensions or source indexes are invalid, a source pixel format is
    /// unsupported, or frame allocation, import, or GPU rendering fails.
    pub fn render(
        &mut self,
        sources: &[CompositorSource<'_>],
        items: &[CompositorItem],
        output_dimensions: (usize, usize),
    ) -> Result<CVPixelBuffer> {
        let (output_width, output_height) = output_dimensions;
        let output = self.output_pixel_buffer(output_width, output_height)?;
        let output_texture = self.import_pixel_buffer_plane(
            &output,
            "blip composed frame",
            MTLPixelFormat::BGRA8Unorm,
            wgpu::TextureFormat::Bgra8Unorm,
            output_width,
            output_height,
            0,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
            MTLTextureUsage::RenderTarget,
        )?;
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let canvas_size = [usize_to_f32(output_width)?, usize_to_f32(output_height)?];

        let mut source_textures = Vec::with_capacity(sources.len());
        let mut content_rects = Vec::with_capacity(sources.len());
        for source in sources {
            source_textures.push(self.import_source(source.pixel_buffer)?);
            content_rects.push(gpu_content_rect(
                source.content_rect,
                source.pixel_buffer.get_width(),
                source.pixel_buffer.get_height(),
            )?);
        }
        let source_views = source_textures
            .iter()
            .map(|textures| {
                (
                    textures
                        .frame
                        .create_view(&wgpu::TextureViewDescriptor::default()),
                    textures.chroma.as_ref().map(|texture| {
                        texture.create_view(&wgpu::TextureViewDescriptor::default())
                    }),
                    textures.content_kind,
                )
            })
            .collect::<Vec<_>>();
        let dummy_view = self
            .dummy_texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut settings_buffers = Vec::with_capacity(items.len());
        for item in items {
            let (content_rect, color, content_kind) = match item.content {
                CompositorItemContent::Source(source_index) => {
                    let content_rect = *content_rects.get(source_index).ok_or_else(|| {
                        anyhow!(
                            "compositor item source index {source_index} is out of bounds for {} sources",
                            sources.len()
                        )
                    })?;
                    let content_kind = source_views
                        .get(source_index)
                        .map(|(_, _, content_kind)| *content_kind)
                        .ok_or_else(|| {
                            anyhow!("compositor source view {source_index} is unavailable")
                        })?;
                    (content_rect, [0.0; 4], content_kind)
                }
                CompositorItemContent::Color(color) => ([0.0, 0.0, 1.0, 1.0], color, 1.0),
                CompositorItemContent::Gradient { start, end } => (end, start, 6.0),
                CompositorItemContent::Checkerboard { first, second } => (second, first, 7.0),
            };
            let corner_radius = item.transform.clamped_corner_radius(canvas_size)
                * if item.transform.squircle { -1.0 } else { 1.0 };
            let box_shadow = item
                .transform
                .box_shadow
                .unwrap_or_else(|| BoxShadow::new([0.0; 2], [0.0; 4]));
            let settings = shader::CompositorSettings::new(
                content_rect,
                transform_data(item.transform),
                [canvas_size[0], canvas_size[1], corner_radius, content_kind],
                color,
                [
                    box_shadow.offset[0],
                    box_shadow.offset[1],
                    box_shadow.blur_radius.max(0.0),
                    box_shadow.spread_radius,
                ],
                box_shadow.color,
            );
            settings_buffers.push(self.device.create_buffer_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("blip compositor item settings"),
                    contents: bytemuck::bytes_of(&settings),
                    usage: wgpu::BufferUsages::UNIFORM,
                },
            ));
        }
        let bind_groups = items
            .iter()
            .zip(&settings_buffers)
            .map(|(item, settings_buffer)| {
                let (source_view, chroma_view) = match item.content {
                    CompositorItemContent::Source(source_index) => {
                        let (frame, chroma, _) =
                            source_views.get(source_index).ok_or_else(|| {
                                anyhow!("compositor source view {source_index} is unavailable")
                            })?;
                        (frame, chroma.as_ref().unwrap_or(&dummy_view))
                    }
                    CompositorItemContent::Color(_)
                    | CompositorItemContent::Gradient { .. }
                    | CompositorItemContent::Checkerboard { .. } => (&dummy_view, &dummy_view),
                };
                Ok(shader::WgpuBindGroup0::from_bindings(
                    &self.device,
                    shader::WgpuBindGroup0Entries::new(shader::WgpuBindGroup0EntriesParams {
                        settings: settings_buffer.as_entire_buffer_binding(),
                        frame: source_view,
                        frame_sampler: &self.sampler,
                        frame_chroma: chroma_view,
                    }),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("blip frame compositor"),
            });
        self.render_frame(&mut encoder, &output_view, &bind_groups);
        let submission = self.queue.submit([encoder.finish()]);
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
            .context("failed while waiting for frame composition")?;
        Ok(output)
    }

    /// Renders all items as one layer, then transforms that combined output around `center`.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::render`].
    pub fn render_with_output_transform(
        &mut self,
        sources: &[CompositorSource<'_>],
        items: &[CompositorItem],
        output_dimensions: (usize, usize),
        transform: OutputTransform,
    ) -> Result<CVPixelBuffer> {
        let combined = self.render(sources, items, output_dimensions)?;
        let scale = if transform.scale.is_finite() {
            transform.scale.max(1.0)
        } else {
            1.0
        };
        let focus = transform.center.map(|value| {
            if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                0.5
            }
        });
        if scale == 1.0 {
            return Ok(combined);
        }

        let output_transform = transformed_output_item(OutputTransform {
            center: focus,
            scale,
        });
        self.render(
            &[CompositorSource {
                pixel_buffer: &combined,
                content_rect: None,
            }],
            &[CompositorItem {
                content: CompositorItemContent::Source(0),
                transform: output_transform,
            }],
            output_dimensions,
        )
    }

    fn render_frame(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        bind_groups: &[shader::WgpuBindGroup0],
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blip frame composition"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        for bind_group in bind_groups {
            bind_group.set(&mut pass);
            pass.draw(0..6, 0..1);
        }
    }

    fn import_source(&self, pixel_buffer: &CVPixelBuffer) -> Result<SourceTextures> {
        let usage = wgpu::TextureUsages::TEXTURE_BINDING;
        let metal_usage = MTLTextureUsage::ShaderRead;
        let format = pixel_buffer.get_pixel_format();
        if format == kCVPixelFormatType_32BGRA {
            Ok(SourceTextures {
                frame: self.import_pixel_buffer_plane(
                    pixel_buffer,
                    "blip source BGRA frame",
                    MTLPixelFormat::BGRA8Unorm,
                    wgpu::TextureFormat::Bgra8Unorm,
                    pixel_buffer.get_width(),
                    pixel_buffer.get_height(),
                    0,
                    usage,
                    metal_usage,
                )?,
                chroma: None,
                content_kind: 0.0,
            })
        } else if format == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
            || format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
        {
            if pixel_buffer.get_plane_count() != 2 {
                return Err(anyhow!("NV12 camera frame does not contain two planes"));
            }
            Ok(SourceTextures {
                frame: self.import_pixel_buffer_plane(
                    pixel_buffer,
                    "blip source NV12 luma",
                    MTLPixelFormat::R8Unorm,
                    wgpu::TextureFormat::R8Unorm,
                    pixel_buffer.get_width_of_plane(0),
                    pixel_buffer.get_height_of_plane(0),
                    0,
                    usage,
                    metal_usage,
                )?,
                chroma: Some(self.import_pixel_buffer_plane(
                    pixel_buffer,
                    "blip source NV12 chroma",
                    MTLPixelFormat::RG8Unorm,
                    wgpu::TextureFormat::Rg8Unorm,
                    pixel_buffer.get_width_of_plane(1),
                    pixel_buffer.get_height_of_plane(1),
                    1,
                    usage,
                    metal_usage,
                )?),
                content_kind: if pixel_buffer.get_height() < 720 {
                    if format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange {
                        5.0
                    } else {
                        4.0
                    }
                } else if format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange {
                    3.0
                } else {
                    2.0
                },
            })
        } else {
            Err(anyhow!("unsupported source pixel format {format:#010x}"))
        }
    }

    fn output_pixel_buffer(&mut self, width: usize, height: usize) -> Result<CVPixelBuffer> {
        let dimensions = (width, height);
        if self
            .output_pool
            .as_ref()
            .is_none_or(|output| output.dimensions != dimensions)
        {
            self.output_pool = Some(OutputPool {
                dimensions,
                pool: output_pixel_buffer_pool(width, height)?,
            });
        }
        self.output_pool
            .as_ref()
            .ok_or_else(|| anyhow!("compositor output pool is unavailable"))?
            .pool
            .create_pixel_buffer()
            .map_err(|status| anyhow!("failed to allocate pooled composed frame ({status})"))
    }

    #[allow(clippy::too_many_arguments)]
    fn import_pixel_buffer_plane(
        &self,
        pixel_buffer: &CVPixelBuffer,
        label: &'static str,
        metal_format: MTLPixelFormat,
        format: wgpu::TextureFormat,
        width: usize,
        height: usize,
        plane: usize,
        usage: wgpu::TextureUsages,
        metal_usage: MTLTextureUsage,
    ) -> Result<wgpu::Texture> {
        let width = u32::try_from(width).context("frame width exceeds u32")?;
        let height = u32::try_from(height).context("frame height exceeds u32")?;
        let usage_value = CFNumber::from(i64::try_from(metal_usage.bits()).unwrap_or(i64::MAX));
        let attributes = CFDictionary::from_CFType_pairs(&[(
            CFString::from(core_video::metal_texture::CVMetalTextureKeys::Usage),
            usage_value.as_CFType(),
        )]);
        let cv_texture = self
            .texture_cache
            .create_texture_from_image(
                pixel_buffer.as_concrete_TypeRef(),
                Some(&attributes),
                metal_format,
                usize::try_from(width).context("texture width exceeds usize")?,
                usize::try_from(height).context("texture height exceeds usize")?,
                plane,
            )
            .map_err(|status| anyhow!("failed to import frame into Metal ({status})"))?;
        let hal_texture = hal_texture(&cv_texture, width, height)?;
        let descriptor = wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        };
        // SAFETY: The CVMetalTexture was created using this wgpu device's Metal device, and its
        // plane shape, format, and declared usage match the descriptor.
        Ok(unsafe {
            self.device
                .create_texture_from_hal::<MetalApi>(hal_texture, &descriptor)
        })
    }
}

fn transformed_output_item(transform: OutputTransform) -> ItemTransform {
    let center = [
        transform.center[0] + transform.scale * (0.5 - transform.center[0]),
        transform.center[1] + transform.scale * (0.5 - transform.center[1]),
    ];
    ItemTransform::new(center, [transform.scale, transform.scale])
}

fn transform_data(transform: ItemTransform) -> [f32; 4] {
    [
        transform.center[0],
        transform.center[1],
        transform.size[0],
        transform.size[1],
    ]
}

#[allow(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation
)]
fn gpu_content_rect(
    content_rect: Option<ContentRect>,
    width: usize,
    height: usize,
) -> Result<[f32; 4]> {
    let width = f64::from(u32::try_from(width).context("frame width exceeds u32")?);
    let height = f64::from(u32::try_from(height).context("frame height exceeds u32")?);
    let rect = content_rect.unwrap_or(ContentRect {
        x: 0.0,
        y: 0.0,
        width,
        height,
    });
    let x = rect.x.clamp(0.0, (width - 1.0).max(0.0));
    let y = rect.y.clamp(0.0, (height - 1.0).max(0.0));
    let rect_width = rect.width.clamp(1.0, width - x);
    let rect_height = rect.height.clamp(1.0, height - y);
    Ok([x as f32, y as f32, rect_width as f32, rect_height as f32])
}

#[allow(clippy::as_conversions, clippy::cast_precision_loss)]
fn usize_to_f32(value: usize) -> Result<f32> {
    Ok(u32::try_from(value).context("frame dimension exceeds u32")? as f32)
}

fn output_pixel_buffer_pool(width: usize, height: usize) -> Result<CVPixelBufferPool> {
    let surface_properties = CFDictionary::<CFString, CFType>::from_CFType_pairs(&[]);
    let attributes = CFDictionary::from_CFType_pairs(&[
        (
            CFString::from(CVPixelBufferKeys::PixelFormatType),
            CFNumber::from(i64::from(kCVPixelFormatType_32BGRA)).as_CFType(),
        ),
        (
            CFString::from(CVPixelBufferKeys::Width),
            CFNumber::from(i64::try_from(width).unwrap_or(i64::MAX)).as_CFType(),
        ),
        (
            CFString::from(CVPixelBufferKeys::Height),
            CFNumber::from(i64::try_from(height).unwrap_or(i64::MAX)).as_CFType(),
        ),
        (
            CFString::from(CVPixelBufferKeys::IOSurfaceProperties),
            surface_properties.as_CFType(),
        ),
        (
            CFString::from(CVPixelBufferKeys::MetalCompatibility),
            CFBoolean::true_value().as_CFType(),
        ),
    ]);
    let pool_attributes = CFDictionary::from_CFType_pairs(&[(
        CFString::from(CVPixelBufferPoolKeys::MinimumBufferCount),
        CFNumber::from(3_i32).as_CFType(),
    )]);
    CVPixelBufferPool::new(Some(&pool_attributes), Some(&attributes))
        .map_err(|status| anyhow!("failed to create composed frame pool ({status})"))
}

fn hal_texture(
    texture: &CVMetalTexture,
    width: u32,
    height: u32,
) -> Result<wgpu::hal::metal::Texture> {
    // SAFETY: CoreVideo returns a live MTLTexture retained by `texture`. We retain the
    // protocol object separately before transferring it into wgpu-hal.
    let retained = unsafe {
        let raw = CVMetalTextureGetTexture(texture.as_concrete_TypeRef());
        let object = Retained::<AnyObject>::retain(raw.cast())
            .ok_or_else(|| anyhow!("CoreVideo returned no Metal texture"))?;
        Retained::cast_unchecked::<ProtocolObject<dyn MTLTexture>>(object)
    };
    // SAFETY: The retained object is a 2D BGRA texture with one layer and mip level.
    Ok(unsafe {
        HalMetalDevice::texture_from_raw(
            retained,
            wgpu::TextureFormat::Bgra8Unorm,
            MTLTextureType::Type2D,
            1,
            1,
            wgpu::hal::CopyExtent {
                width,
                height,
                depth: 1,
            },
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{OutputTransform, transformed_output_item};

    #[test]
    fn output_transform_keeps_focal_point_fixed() {
        let focus = [0.25, 0.75];
        let transform = transformed_output_item(OutputTransform {
            center: focus,
            scale: 3.0,
        });
        let top_left = [
            transform.center[0] - transform.size[0] * 0.5,
            transform.center[1] - transform.size[1] * 0.5,
        ];
        let transformed_focus = [
            top_left[0] + focus[0] * transform.size[0],
            top_left[1] + focus[1] * transform.size[1],
        ];
        assert_eq!(transformed_focus, focus);
    }

    #[test]
    fn identity_output_transform_is_full_frame() {
        let transform = transformed_output_item(OutputTransform::IDENTITY);
        assert_eq!(transform.center, [0.5, 0.5]);
        assert_eq!(transform.size, [1.0, 1.0]);
    }
}
