//! GPU compositor (wgpu: Metal on macOS, DirectX 12 on Windows).
//!
//! Sources arrive as NV12 frames and are uploaded as two textures (Y and
//! CbCr); the shader converts to RGB, applies each source's crop, mirror,
//! colour and chroma key, and draws it with rounded corners, border and
//! shadow where the scene doesn't fill the screen. Wipes, pushes and dips
//! render both scenes off-screen and combine them. Graphics go on top.
//! The canvas is then converted back to NV12 for the encoders (at the
//! program size and, when streaming, at the stream size), and scaled to
//! RGBA for the monitors.

use super::graphics::GraphicLayer;
use super::switcher::Layout;
use crate::config::{parse_color, FrameStyle, SourceLook, TransitionMode};
use crate::media::{Ns, Pixels, VideoFrame};
use crate::scene::PadState;
use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use wgpu::util::DeviceExt;

/// Uniforms for one draw (see `U` in compositor.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Quad {
    rect: [f32; 4],
    uv: [f32; 4],
    params: [f32; 4],
    px: [f32; 4],
    style: [f32; 4],
    style2: [f32; 4],
    border: [f32; 4],
    color: [f32; 4],
    key: [f32; 4],
    key2: [f32; 4],
    keyrgb: [f32; 4],
    trans: [f32; 4],
    dip: [f32; 4],
}

impl Quad {
    fn full(alpha: f32, taps: f32, foot: [f32; 2]) -> Quad {
        Quad { rect: [-1.0, 1.0, 1.0, -1.0], uv: [0.0, 0.0, 1.0, 1.0], params: [alpha, taps, foot[0], foot[1]], ..Default::default() }
    }
}

/// A source's look, ready for the shader.
#[derive(Clone, Copy)]
struct LookGpu {
    uv: [f32; 4],
    color: [f32; 4],
    key: [f32; 4],
    key2: [f32; 4],
    keyrgb: [f32; 4],
}

impl Default for LookGpu {
    fn default() -> Self {
        LookGpu::from(&SourceLook::default())
    }
}

fn rgb01(s: &str, fallback: u32) -> [f32; 3] {
    let c = parse_color(s).unwrap_or(fallback);
    [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0]
}

impl From<&SourceLook> for LookGpu {
    fn from(l: &SourceLook) -> Self {
        let r = l.crop_rect();
        let uv = if l.mirror { [r[2], r[1], r[0], r[3]] } else { r };
        let k = rgb01(&l.key_color, 0x00d040);
        // Key colour's CbCr as the shader samples it (BT.709 video range).
        let cb = (128.0 + 224.0 * (-0.1146 * k[0] - 0.3854 * k[1] + 0.5 * k[2])) / 255.0;
        let cr = (128.0 + 224.0 * (0.5 * k[0] - 0.4542 * k[1] - 0.0458 * k[2])) / 255.0;
        LookGpu {
            uv,
            color: [l.brightness.clamp(-1.0, 1.0), l.contrast.clamp(0.0, 3.0), l.saturation.clamp(0.0, 3.0), 0.0],
            key: [cb, cr, l.key_similarity.clamp(0.0, 1.0) * 0.5, l.key_smoothness.clamp(0.0, 1.0) * 0.5],
            key2: [l.key_spill.clamp(0.0, 1.0), if l.chroma_key { 1.0 } else { 0.0 }, if l.remove_background { 1.0 } else { 0.0 }, 0.0],
            keyrgb: [k[0], k[1], k[2], 0.0],
        }
    }
}

/// Frame style in canvas pixels.
#[derive(Clone, Copy, Default)]
struct StyleGpu {
    radius: f32,
    border: f32,
    border_rgb: [f32; 3],
    shadow: f32,
}

impl StyleGpu {
    fn from(s: &FrameStyle) -> Self {
        StyleGpu { radius: s.corner_radius.max(0.0), border: s.border_width.max(0.0), border_rgb: rgb01(&s.border_color, 0xffffff), shadow: s.shadow.clamp(0.0, 1.0) }
    }
}

/// Which copy of a source's uniforms a draw uses (a source can appear in
/// several renders of one frame).
const SLOT_A: usize = 0;
const SLOT_B: usize = 1;
const SLOT_PREVIEW: usize = 2;

const COPY_ALIGN: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

fn align(n: u32) -> u32 {
    n.div_ceil(COPY_ALIGN) * COPY_ALIGN
}

/// Taps per axis to cover a reduction factor, and the footprint in uv.
fn reduction(src: (u32, u32), dst: (u32, u32)) -> (f32, [f32; 2]) {
    let fx = src.0 as f32 / dst.0 as f32;
    let fy = src.1 as f32 / dst.1 as f32;
    let f = fx.max(fy);
    let taps = if f <= 1.25 { 1.0 } else { f.ceil().min(4.0) };
    (taps, [1.0 / dst.0 as f32, 1.0 / dst.1 as f32])
}

/// A render target that can be read back.
struct Target {
    tex: wgpu::Texture,
    view: wgpu::TextureView,
    buf: wgpu::Buffer,
    width: u32,
    height: u32,
    bpp: u32,
    stride: u32,
}

impl Target {
    fn new(dev: &wgpu::Device, label: &str, width: u32, height: u32, format: wgpu::TextureFormat, bpp: u32) -> Target {
        let tex = dev.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let stride = align(width * bpp);
        let buf = dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (stride * height) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view = tex.create_view(&Default::default());
        Target { tex, view, buf, width, height, bpp, stride }
    }

    fn copy(&self, enc: &mut wgpu::CommandEncoder) {
        enc.copy_texture_to_buffer(
            self.tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.stride), rows_per_image: Some(self.height) },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
    }
}

/// NV12 output (Y + CbCr targets).
struct NvOut {
    y: Target,
    uv: Target,
    y_bind: wgpu::BindGroup,
    uv_bind: wgpu::BindGroup,
}

struct StreamCanvas {
    target: Target,
    /// Draws the program canvas onto it.
    copy: wgpu::BindGroup,
    out: NvOut,
}

struct SourceGpu {
    width: u32,
    height: u32,
    y: wgpu::Texture,
    uv: wgpu::Texture,
    /// Uniforms per slot (SLOT_A, SLOT_B, SLOT_PREVIEW).
    slots: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    thumb: Option<(Target, wgpu::Buffer, wgpu::BindGroup)>,
    serial: Option<u64>,
    /// Person mask (background removal): texture and size, and the
    /// capture time of the mask in it.
    mask: Option<(wgpu::Texture, (u32, u32))>,
    mask_pts: Option<Ns>,
}

struct GraphicGpu {
    _tex: wgpu::Texture,
    size: (u32, u32),
    /// Uniforms for the program and for the preview monitor.
    program: (wgpu::Buffer, wgpu::BindGroup),
    preview: (wgpu::Buffer, wgpu::BindGroup),
    used: bool,
}

/// An off-screen whole-scene picture (for wipes, pushes and dips).
struct SceneTex {
    _tex: wgpu::Texture,
    view: wgpu::TextureView,
}

/// Results of one render.
#[derive(Default)]
pub struct Rendered {
    pub program: Option<VideoFrame>,
    pub stream: Option<VideoFrame>,
    /// RGBA, tightly packed.
    pub preview: Option<(u32, u32, Vec<u8>)>,
    /// The preview bus (next scene), RGBA.
    pub preview_bus: Option<(u32, u32, Vec<u8>)>,
    pub thumbs: Vec<(usize, u32, u32, Vec<u8>)>,
}

/// What to produce this time.
#[derive(Default)]
pub struct Wants {
    pub program: bool,
    pub stream: Option<(u32, u32)>,
    pub preview: bool,
    pub preview_bus: bool,
    /// Sources whose thumbnails should be refreshed.
    pub thumbs: Vec<usize>,
}

pub struct Compositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pub adapter_name: String,
    canvas: Target,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// 1×1 white (no mask).
    white: wgpu::TextureView,
    yuv_pipe: wgpu::RenderPipeline,
    rgba_pipe: wgpu::RenderPipeline,
    out_y_pipe: wgpu::RenderPipeline,
    out_uv_pipe: wgpu::RenderPipeline,
    layer_pipe: wgpu::RenderPipeline,
    trans_pipe: wgpu::RenderPipeline,
    sources: Vec<SourceGpu>,
    looks: Vec<LookGpu>,
    style: StyleGpu,
    background: [f64; 3],
    bg_image: Option<(wgpu::Texture, wgpu::BindGroup)>,
    program_out: NvOut,
    stream_out: Option<NvOut>,
    /// The stream's own canvas, for graphics only the stream gets
    /// (captions): a copy of the program with those on top.
    stream_canvas: Option<StreamCanvas>,
    preview: (Target, wgpu::BindGroup),
    preview_bus: Option<Target>,
    thumb_width: u32,
    graphics: HashMap<(usize, u64), GraphicGpu>,
    /// Off-screen scenes and the uniforms that combine them.
    scene_a: Option<SceneTex>,
    scene_b: Option<SceneTex>,
    trans: Option<(wgpu::Buffer, wgpu::BindGroup)>,
}

/// Create a GPU device for compositing.
pub fn create_device() -> Result<(wgpu::Device, wgpu::Queue, String)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        ..Default::default()
    }))
    .map_err(|e| anyhow!("no usable GPU: {e}"))?;
    let info = adapter.get_info();
    let name = format!("{} ({:?})", info.name, info.backend);
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("cutaway compositor"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .context("opening the GPU")?;
    // A GPU error while running is logged rather than ending the app.
    device.on_uncaptured_error(Arc::new(|e| eprintln!("GPU error: {e}")));
    Ok((device, queue, name))
}

pub struct CompositorSettings {
    pub canvas: (u32, u32),
    pub background_rgb: [u8; 3],
    pub background_image: Option<image::RgbaImage>,
    pub preview_width: u32,
    pub thumb_width: u32,
}

impl Compositor {
    pub fn new(s: &CompositorSettings) -> Result<Compositor> {
        let (device, queue, adapter_name) = create_device()?;
        // Anything the GPU rejects while setting up becomes an error here.
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let c = Self::build(s, device, queue, adapter_name)?;
        if let Some(e) = pollster::block_on(scope.pop()) {
            bail!("the GPU rejected the compositor setup: {e}");
        }
        Ok(c)
    }

    fn build(s: &CompositorSettings, device: wgpu::Device, queue: wgpu::Queue, adapter_name: String) -> Result<Compositor> {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compositor"),
            source: wgpu::ShaderSource::Wgsl(include_str!("compositor.wgsl").into()),
        });
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("quad"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                tex_entry(2),
                tex_entry(3),
                tex_entry(4),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quad"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipe = |fs: &str, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let rgba = wgpu::TextureFormat::Rgba8Unorm;
        // Colour blends over; alpha accumulates so the canvas stays opaque.
        let blend = Some(wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
        });
        let yuv_pipe = pipe("fs_yuv", rgba, blend);
        let rgba_pipe = pipe("fs_rgba", rgba, blend);
        let layer_pipe = pipe("fs_layer", rgba, blend);
        let trans_pipe = pipe("fs_trans", rgba, None);
        let out_y_pipe = pipe("fs_out_y", wgpu::TextureFormat::R8Unorm, None);
        let out_uv_pipe = pipe("fs_out_uv", wgpu::TextureFormat::Rg8Unorm, None);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let (cw, ch) = s.canvas;
        let canvas = Target::new(&device, "canvas", cw, ch, rgba, 4);
        let white = device
            .create_texture_with_data(
                &queue,
                &wgpu::TextureDescriptor {
                    label: Some("white"),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &[255],
            )
            .create_view(&Default::default());
        let mut c = Compositor {
            program_out: NvOut::placeholder(&device),
            preview: (Target::new(&device, "preview", 2, 2, rgba, 4), Self::dummy_bind(&device, &layout, &sampler)),
            white,
            device,
            queue,
            adapter_name,
            canvas,
            layout,
            sampler,
            yuv_pipe,
            rgba_pipe,
            out_y_pipe,
            out_uv_pipe,
            layer_pipe,
            trans_pipe,
            sources: Vec::new(),
            looks: Vec::new(),
            style: StyleGpu::default(),
            preview_bus: None,
            graphics: HashMap::new(),
            scene_a: None,
            scene_b: None,
            trans: None,
            background: [s.background_rgb[0] as f64 / 255.0, s.background_rgb[1] as f64 / 255.0, s.background_rgb[2] as f64 / 255.0],
            bg_image: None,
            stream_out: None,
            stream_canvas: None,
            thumb_width: s.thumb_width.max(32),
        };
        c.program_out = c.nv_out(cw, ch);
        c.set_preview_width(s.preview_width);
        if let Some(img) = &s.background_image {
            let tex = c.device.create_texture_with_data(
                &c.queue,
                &wgpu::TextureDescriptor {
                    label: Some("background"),
                    size: wgpu::Extent3d { width: img.width(), height: img.height(), depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: rgba,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                img.as_raw(),
            );
            let view = tex.create_view(&Default::default());
            let (taps, foot) = reduction((img.width(), img.height()), (cw, ch));
            let bind = c.bind(Quad::full(1.0, taps, foot), &view, &view).1;
            c.bg_image = Some((tex, bind));
        }
        Ok(c)
    }

    fn dummy_bind(dev: &wgpu::Device, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler) -> wgpu::BindGroup {
        let tex = dev.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let buf = dev.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&Quad::full(1.0, 1.0, [0.0, 0.0])),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        dev.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&view) },
            ],
        })
    }

    fn bind(&self, quad: Quad, a: &wgpu::TextureView, b: &wgpu::TextureView) -> (wgpu::Buffer, wgpu::BindGroup) {
        self.bind_mask(quad, a, b, &self.white)
    }

    /// Uniforms and textures for one draw, with a mask texture.
    fn bind_mask(&self, quad: Quad, a: &wgpu::TextureView, b: &wgpu::TextureView, mask: &wgpu::TextureView) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&quad),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(a) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(b) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(mask) },
            ],
        });
        (buf, bind)
    }

    fn nv_out(&self, w: u32, h: u32) -> NvOut {
        self.nv_out_from(&self.canvas.view, w, h)
    }

    /// NV12 output of size w×h from a canvas-sized picture.
    fn nv_out_from(&self, view: &wgpu::TextureView, w: u32, h: u32) -> NvOut {
        let y = Target::new(&self.device, "out y", w, h, wgpu::TextureFormat::R8Unorm, 1);
        let uv = Target::new(&self.device, "out uv", w / 2, h / 2, wgpu::TextureFormat::Rg8Unorm, 2);
        let canvas = (self.canvas.width, self.canvas.height);
        let (ty, fy) = reduction(canvas, (w, h));
        let (tc, fc) = reduction(canvas, (w / 2, h / 2));
        let y_bind = self.bind(Quad::full(1.0, ty, fy), view, view).1;
        let uv_bind = self.bind(Quad::full(1.0, tc, fc), view, view).1;
        NvOut { y, uv, y_bind, uv_bind }
    }

    pub fn canvas_size(&self) -> (u32, u32) {
        (self.canvas.width, self.canvas.height)
    }

    /// Register a source (index = order of calls).
    pub fn add_source(&mut self, width: u32, height: u32) -> usize {
        let s = self.make_source(width, height);
        self.sources.push(s);
        self.sources.len() - 1
    }

    fn make_source(&self, width: u32, height: u32) -> SourceGpu {
        let (width, height) = (width.max(2) & !1, height.max(2) & !1);
        let mk = |label: &str, w: u32, h: u32, format| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let y = mk("src y", width, height, wgpu::TextureFormat::R8Unorm);
        let uv = mk("src uv", width / 2, height / 2, wgpu::TextureFormat::Rg8Unorm);
        let (yv, uvv) = (y.create_view(&Default::default()), uv.create_view(&Default::default()));
        let slots = (0..3).map(|_| self.bind(Quad::full(0.0, 1.0, [0.0, 0.0]), &yv, &uvv)).collect();
        let tw = self.thumb_width.min(width) & !1;
        let th = ((tw as u64 * height as u64 / width as u64) as u32).max(2) & !1;
        let thumb = Target::new(&self.device, "thumb", tw, th, wgpu::TextureFormat::Rgba8Unorm, 4);
        let (tbuf, tbind) = self.bind(Quad::full(1.0, 1.0, [0.0, 0.0]), &yv, &uvv);
        SourceGpu { width, height, y, uv, slots, thumb: Some((thumb, tbuf, tbind)), serial: None, mask: None, mask_pts: None }
    }

    /// A person mask for source `index` (from a picture captured at `pts`):
    /// where it's 0 the background shows through. Skipped if already there.
    pub fn upload_mask(&mut self, index: usize, pts: Ns, mask: &crate::platform::vision::Mask) {
        let Some(s) = self.sources.get(index) else { return };
        if s.mask_pts == Some(pts) || mask.width == 0 || mask.height == 0 || mask.data.len() < (mask.width * mask.height) as usize {
            return;
        }
        let size = (mask.width, mask.height);
        if s.mask.as_ref().map(|m| m.1) != Some(size) {
            let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("mask"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let mv = tex.create_view(&Default::default());
            let (yv, uvv) = (s.y.create_view(&Default::default()), s.uv.create_view(&Default::default()));
            let slots = (0..3).map(|_| self.bind_mask(Quad::full(0.0, 1.0, [0.0, 0.0]), &yv, &uvv, &mv)).collect();
            let s = &mut self.sources[index];
            s.slots = slots;
            s.mask = Some((tex, size));
        }
        let s = &mut self.sources[index];
        s.mask_pts = Some(pts);
        let tex = &s.mask.as_ref().unwrap().0;
        self.queue.write_texture(
            tex.as_image_copy(),
            &mask.data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.0), rows_per_image: Some(size.1) },
            wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        );
    }

    /// Stop using source `index`'s mask (whole picture shown).
    pub fn clear_mask(&mut self, index: usize) {
        if let Some(s) = self.sources.get_mut(index) {
            s.mask_pts = None;
        }
    }

    /// Crop, mirror, colour and key for each source; corners, border and
    /// shadow for frames that don't fill the screen.
    pub fn set_looks(&mut self, looks: &[SourceLook], style: &FrameStyle) {
        self.looks = looks.iter().map(LookGpu::from).collect();
        self.style = StyleGpu::from(style);
    }

    /// Size of the program monitor picture (bigger when a full-screen
    /// output is open).
    pub fn set_preview_width(&mut self, width: u32) {
        let (cw, ch) = (self.canvas.width, self.canvas.height);
        let pw = (width.clamp(64, cw)) & !1;
        if self.preview.0.width == pw {
            return;
        }
        let ph = ((pw as u64 * ch as u64 / cw as u64) as u32).max(2) & !1;
        let preview = Target::new(&self.device, "preview", pw, ph, wgpu::TextureFormat::Rgba8Unorm, 4);
        let (taps, foot) = reduction((cw, ch), (pw, ph));
        let bind = self.bind(Quad::full(1.0, taps, foot), &self.canvas.view, &self.canvas.view).1;
        self.preview = (preview, bind);
        self.preview_bus = None;
    }

    pub fn preview_width(&self) -> u32 {
        self.preview.0.width
    }

    /// Uniforms for source `i` drawn at `st` on a target of `target` pixels
    /// (the canvas, or the smaller preview).
    fn layer_quad(&self, i: usize, st: &PadState, target: (u32, u32)) -> Quad {
        let (cw, ch) = (self.canvas.width as f32, self.canvas.height as f32);
        let k = target.0 as f32 / cw;
        let (tw, th) = (target.0 as f32, target.1 as f32);
        let look = self.looks.get(i).copied().unwrap_or_default();
        let px = [st.x as f32 * k, st.y as f32 * k, (st.x + st.w) as f32 * k, (st.y + st.h) as f32 * k];
        // Style fades in as a frame shrinks from full screen.
        let gap = 1.0 - (st.w as f32 / cw).max(st.h as f32 / ch);
        let amount = (gap / 0.05).clamp(0.0, 1.0);
        let s = self.style;
        let shadow_off = [0.004 * cw * k, 0.006 * cw * k];
        let blur = 0.012 * cw * k;
        let margin = if s.shadow > 0.0 && amount > 0.0 { shadow_off[0].max(shadow_off[1]) + blur * 1.5 } else { 0.0 } + 1.0;
        let to_clip = |x: f32, y: f32| [x / tw * 2.0 - 1.0, 1.0 - y / th * 2.0];
        let a = to_clip(px[0] - margin, px[1] - margin);
        let b = to_clip(px[2] + margin, px[3] + margin);
        Quad {
            rect: [a[0], a[1], b[0], b[1]],
            uv: look.uv,
            params: [st.alpha as f32, 1.0, 0.0, 0.0],
            px,
            style: [s.radius * k, s.border * k, s.shadow, amount],
            style2: [shadow_off[0], shadow_off[1], blur, 0.0],
            border: [s.border_rgb[0], s.border_rgb[1], s.border_rgb[2], 0.0],
            color: look.color,
            key: look.key,
            // Background removal once there's a mask.
            key2: [look.key2[0], look.key2[1], if look.key2[2] > 0.5 && self.sources[i].mask_pts.is_some() { 1.0 } else { 0.0 }, look.key2[3]],
            keyrgb: look.keyrgb,
            ..Default::default()
        }
    }

    /// Sources to draw for a layout, bottom first, with their uniforms written to `slot`.
    fn prepare_layers(&self, states: &[PadState], slot: usize, target: (u32, u32)) -> Vec<usize> {
        let mut order: Vec<usize> = (0..states.len().min(self.sources.len()))
            .filter(|&i| states[i].alpha > 0.001 && self.sources[i].serial.is_some())
            .collect();
        order.sort_by_key(|&i| states[i].zorder);
        for &i in &order {
            let q = self.layer_quad(i, &states[i], target);
            self.queue.write_buffer(&self.sources[i].slots[slot].0, 0, bytemuck::bytes_of(&q));
        }
        order
    }

    fn scene_tex(&self, label: &str) -> SceneTex {
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: self.canvas.width, height: self.canvas.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        SceneTex { _tex: tex, view }
    }

    /// Upload graphics that are new or changed; forget ones no longer shown.
    fn prepare_graphics(&mut self, layers: &[GraphicLayer]) {
        for g in self.graphics.values_mut() {
            g.used = false;
        }
        for l in layers {
            if !self.graphics.contains_key(&l.key) {
                let img = &l.image;
                let tex = self.device.create_texture_with_data(
                    &self.queue,
                    &wgpu::TextureDescriptor {
                        label: Some("graphic"),
                        size: wgpu::Extent3d { width: img.width(), height: img.height(), depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                    wgpu::util::TextureDataOrder::LayerMajor,
                    img.as_raw(),
                );
                let view = tex.create_view(&Default::default());
                let program = self.bind(Quad::full(0.0, 1.0, [0.0, 0.0]), &view, &view);
                let preview = self.bind(Quad::full(0.0, 1.0, [0.0, 0.0]), &view, &view);
                self.graphics.insert(l.key, GraphicGpu { _tex: tex, size: (img.width(), img.height()), program, preview, used: false });
            }
            let g = self.graphics.get_mut(&l.key).unwrap();
            g.used = true;
            let [x, y, w, h] = l.rect;
            for (buf, target_w) in [(&g.program.0, self.canvas.width), (&g.preview.0, self.preview.0.width)] {
                let dst = ((w * target_w as f32).max(1.0) as u32, (h * target_w as f32 * self.canvas.height as f32 / self.canvas.width as f32).max(1.0) as u32);
                let (taps, foot) = reduction(g.size, dst);
                let q = Quad {
                    rect: [x * 2.0 - 1.0, 1.0 - y * 2.0, (x + w) * 2.0 - 1.0, 1.0 - (y + h) * 2.0],
                    params: [l.alpha, taps, foot[0], foot[1]],
                    uv: [0.0, 0.0, 1.0, 1.0],
                    ..Default::default()
                };
                self.queue.write_buffer(buf, 0, bytemuck::bytes_of(&q));
            }
        }
        self.graphics.retain(|_, g| g.used);
    }

    /// Upload a source's newest frame (skipped if it's already there).
    pub fn upload(&mut self, index: usize, frame: &VideoFrame) {
        let Some(s) = self.sources.get(index) else { return };
        if s.serial == Some(frame.serial) || frame.width < 2 || frame.height < 2 {
            return;
        }
        if frame.width & !1 != s.width || frame.height & !1 != s.height {
            // The device delivers a different size than configured (or
            // switched modes): follow it.
            self.sources[index] = self.make_source(frame.width, frame.height);
        }
        let s = &mut self.sources[index];
        s.serial = Some(frame.serial);
        let (w, h) = (s.width, s.height);
        let queue = &self.queue;
        let (yt, uvt) = (&s.y, &s.uv);
        frame.pixels.with_planes(|p| {
            // A short buffer would be a GPU validation error; skip it.
            let need = |stride: usize, rows: u32, row: usize| stride * (rows as usize - 1) + row;
            if p.y.len() < need(p.y_stride, h, w as usize) || p.uv.len() < need(p.uv_stride, h / 2, w as usize) {
                return;
            }
            queue.write_texture(
                yt.as_image_copy(),
                p.y,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(p.y_stride as u32), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            queue.write_texture(
                uvt.as_image_copy(),
                p.uv,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(p.uv_stride as u32), rows_per_image: Some(h / 2) },
                wgpu::Extent3d { width: w / 2, height: h / 2, depth_or_array_layers: 1 },
            );
        });
    }

    /// Draw the program (and the preview bus) and produce what's wanted.
    pub fn render(&mut self, layout: &Layout, preview_bus: Option<&[PadState]>, graphics: &[GraphicLayer], pts: Ns, serial: u64, wants: &Wants) -> Result<Rendered> {
        let canvas = (self.canvas.width, self.canvas.height);
        if let Some((w, h)) = wants.stream {
            let fits = self.stream_out.as_ref().is_some_and(|o| o.y.width == w && o.y.height == h);
            if !fits {
                self.stream_out = Some(self.nv_out(w, h));
            }
        }
        self.prepare_graphics(graphics);
        let graphic_keys: Vec<(usize, u64)> = graphics.iter().filter(|g| !g.stream_only).map(|g| g.key).collect();
        let stream_keys: Vec<(usize, u64)> = graphics.iter().filter(|g| g.stream_only).map(|g| g.key).collect();
        // Stream-only graphics need the stream's own canvas.
        let use_sc = wants.stream.is_some() && !stream_keys.is_empty();
        if let (true, Some((w, h))) = (use_sc, wants.stream) {
            let fits = self.stream_canvas.as_ref().is_some_and(|sc| sc.out.y.width == w && sc.out.y.height == h);
            if !fits {
                let target = Target::new(&self.device, "stream canvas", self.canvas.width, self.canvas.height, wgpu::TextureFormat::Rgba8Unorm, 4);
                let copy = self.bind(Quad::full(1.0, 1.0, [0.0, 0.0]), &self.canvas.view, &self.canvas.view).1;
                let out = self.nv_out_from(&target.view, w, h);
                self.stream_canvas = Some(StreamCanvas { target, copy, out });
            }
        }

        // Uniforms for this frame.
        enum Plan {
            One(Vec<usize>),
            Two(Vec<usize>, Vec<usize>),
        }
        let plan = match layout {
            Layout::One(states) => Plan::One(self.prepare_layers(states, SLOT_A, canvas)),
            Layout::Two { from, to, mode, t, direction, dip } => {
                let a = self.prepare_layers(from, SLOT_A, canvas);
                let b = self.prepare_layers(to, SLOT_B, canvas);
                if self.scene_a.is_none() {
                    let (sa, sb) = (self.scene_tex("scene a"), self.scene_tex("scene b"));
                    let bind = self.bind(Quad::full(1.0, 1.0, [0.0, 0.0]), &sa.view, &sb.view);
                    self.scene_a = Some(sa);
                    self.scene_b = Some(sb);
                    self.trans = Some(bind);
                }
                let kind = match mode {
                    TransitionMode::Wipe => 2.0,
                    TransitionMode::Push => 3.0,
                    _ => 4.0,
                };
                let q = Quad { trans: [kind, *t, direction[0], direction[1]], dip: [dip[0], dip[1], dip[2], 0.0], ..Quad::full(1.0, 1.0, [0.0, 0.0]) };
                self.queue.write_buffer(&self.trans.as_ref().unwrap().0, 0, bytemuck::bytes_of(&q));
                Plan::Two(a, b)
            }
        };
        let want_bus = wants.preview_bus && preview_bus.is_some();
        let bus_order = match (want_bus, preview_bus) {
            (true, Some(states)) => {
                let size = (self.preview.0.width, self.preview.0.height);
                if self.preview_bus.as_ref().is_none_or(|t| (t.width, t.height) != size) {
                    self.preview_bus = Some(Target::new(&self.device, "preview bus", size.0, size.1, wgpu::TextureFormat::Rgba8Unorm, 4));
                }
                self.prepare_layers(states, SLOT_PREVIEW, size)
            }
            _ => Vec::new(),
        };

        let mut enc = self.device.create_command_encoder(&Default::default());
        let clear = wgpu::Color { r: self.background[0], g: self.background[1], b: self.background[2], a: 1.0 };
        // One scene (background + sources) onto a target.
        let draw_scene = |enc: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, order: &[usize], slot: usize, graphics: Option<bool>| {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some((_, bind)) = &self.bg_image {
                rp.set_pipeline(&self.rgba_pipe);
                rp.set_bind_group(0, bind, &[]);
                rp.draw(0..4, 0..1);
            }
            rp.set_pipeline(&self.layer_pipe);
            for &i in order {
                rp.set_bind_group(0, &self.sources[i].slots[slot].1, &[]);
                rp.draw(0..4, 0..1);
            }
            // Some(true): program graphics; Some(false): preview-size graphics.
            if let Some(program) = graphics {
                rp.set_pipeline(&self.rgba_pipe);
                for k in &graphic_keys {
                    if let Some(g) = self.graphics.get(k) {
                        rp.set_bind_group(0, if program { &g.program.1 } else { &g.preview.1 }, &[]);
                        rp.draw(0..4, 0..1);
                    }
                }
            }
        };
        match &plan {
            Plan::One(order) => draw_scene(&mut enc, &self.canvas.view, order, SLOT_A, Some(true)),
            Plan::Two(a, b) => {
                draw_scene(&mut enc, &self.scene_a.as_ref().unwrap().view, a, SLOT_A, None);
                draw_scene(&mut enc, &self.scene_b.as_ref().unwrap().view, b, SLOT_B, None);
                let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("transition"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.canvas.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                rp.set_pipeline(&self.trans_pipe);
                rp.set_bind_group(0, &self.trans.as_ref().unwrap().1, &[]);
                rp.draw(0..4, 0..1);
                rp.set_pipeline(&self.rgba_pipe);
                for k in &graphic_keys {
                    if let Some(g) = self.graphics.get(k) {
                        rp.set_bind_group(0, &g.program.1, &[]);
                        rp.draw(0..4, 0..1);
                    }
                }
            }
        }
        if want_bus {
            let t = self.preview_bus.as_ref().unwrap();
            draw_scene(&mut enc, &t.view, &bus_order, SLOT_PREVIEW, Some(false));
            t.copy(&mut enc);
        }
        let pass = |enc: &mut wgpu::CommandEncoder, target: &Target, pipe: &wgpu::RenderPipeline, bind: &wgpu::BindGroup| {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(pipe);
            rp.set_bind_group(0, bind, &[]);
            rp.draw(0..4, 0..1);
        };
        let mut reads: Vec<&Target> = Vec::new();
        if want_bus {
            reads.push(self.preview_bus.as_ref().unwrap());
        }
        if wants.program {
            let o = &self.program_out;
            pass(&mut enc, &o.y, &self.out_y_pipe, &o.y_bind);
            pass(&mut enc, &o.uv, &self.out_uv_pipe, &o.uv_bind);
            o.y.copy(&mut enc);
            o.uv.copy(&mut enc);
            reads.push(&o.y);
            reads.push(&o.uv);
        }
        if use_sc {
            let sc = self.stream_canvas.as_ref().unwrap();
            {
                let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("stream canvas"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &sc.target.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                rp.set_pipeline(&self.rgba_pipe);
                rp.set_bind_group(0, &sc.copy, &[]);
                rp.draw(0..4, 0..1);
                for k in &stream_keys {
                    if let Some(g) = self.graphics.get(k) {
                        rp.set_bind_group(0, &g.program.1, &[]);
                        rp.draw(0..4, 0..1);
                    }
                }
            }
            let o = &sc.out;
            pass(&mut enc, &o.y, &self.out_y_pipe, &o.y_bind);
            pass(&mut enc, &o.uv, &self.out_uv_pipe, &o.uv_bind);
            o.y.copy(&mut enc);
            o.uv.copy(&mut enc);
            reads.push(&o.y);
            reads.push(&o.uv);
        } else if wants.stream.is_some() {
            let o = self.stream_out.as_ref().unwrap();
            pass(&mut enc, &o.y, &self.out_y_pipe, &o.y_bind);
            pass(&mut enc, &o.uv, &self.out_uv_pipe, &o.uv_bind);
            o.y.copy(&mut enc);
            o.uv.copy(&mut enc);
            reads.push(&o.y);
            reads.push(&o.uv);
        }
        if wants.preview {
            pass(&mut enc, &self.preview.0, &self.rgba_pipe, &self.preview.1);
            self.preview.0.copy(&mut enc);
            reads.push(&self.preview.0);
        }
        let mut thumb_ids = Vec::new();
        for &i in &wants.thumbs {
            if let Some(s) = self.sources.get(i) {
                if let (Some((t, _, bind)), Some(_)) = (&s.thumb, s.serial) {
                    pass(&mut enc, t, &self.yuv_pipe, bind);
                    t.copy(&mut enc);
                    reads.push(t);
                    thumb_ids.push(i);
                }
            }
        }
        self.queue.submit([enc.finish()]);

        // Read everything back.
        for t in &reads {
            t.buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        }
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow!("GPU: {e}"))?;
        let read = |t: &Target| -> Vec<u8> {
            let data = t.buf.slice(..).get_mapped_range().map(|v| v.to_vec()).unwrap_or_default();
            t.buf.unmap();
            data
        };
        let tight = |t: &Target, data: Vec<u8>| -> Vec<u8> {
            let row = (t.width * t.bpp) as usize;
            if row == t.stride as usize {
                return data;
            }
            let mut out = Vec::with_capacity(row * t.height as usize);
            for r in 0..t.height as usize {
                out.extend_from_slice(&data[r * t.stride as usize..r * t.stride as usize + row]);
            }
            out
        };
        let nv12 = |o: &NvOut| -> VideoFrame {
            let y = read(&o.y);
            let uv = read(&o.uv);
            VideoFrame {
                width: o.y.width,
                height: o.y.height,
                pts,
                pixels: Arc::new(Pixels::Nv12 { y, y_stride: o.y.stride as usize, uv, uv_stride: o.uv.stride as usize }),
                serial,
            }
        };
        let mut out = Rendered::default();
        if want_bus {
            let t = self.preview_bus.as_ref().unwrap();
            let data = read(t);
            out.preview_bus = Some((t.width, t.height, tight(t, data)));
        }
        if wants.program {
            out.program = Some(nv12(&self.program_out));
        }
        if use_sc {
            out.stream = Some(nv12(&self.stream_canvas.as_ref().unwrap().out));
        } else if wants.stream.is_some() {
            out.stream = Some(nv12(self.stream_out.as_ref().unwrap()));
        }
        if wants.preview {
            let t = &self.preview.0;
            let data = read(t);
            out.preview = Some((t.width, t.height, tight(t, data)));
        }
        for i in thumb_ids {
            let t = &self.sources[i].thumb.as_ref().unwrap().0;
            let data = read(t);
            out.thumbs.push((i, t.width, t.height, tight(t, data)));
        }
        Ok(out)
    }
}

impl NvOut {
    fn placeholder(dev: &wgpu::Device) -> NvOut {
        let y = Target::new(dev, "ph", 2, 2, wgpu::TextureFormat::R8Unorm, 1);
        let uv = Target::new(dev, "ph", 2, 2, wgpu::TextureFormat::Rg8Unorm, 2);
        let layout = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[] });
        let bg = dev.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &[] });
        let bg2 = dev.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &[] });
        NvOut { y, uv, y_bind: bg, uv_bind: bg2 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::testsrc::rgb_to_yuv;

    fn solid(w: u32, h: u32, rgb: (f32, f32, f32), serial: u64) -> VideoFrame {
        let (y, u, v) = rgb_to_yuv(rgb.0, rgb.1, rgb.2);
        let mut uv = vec![0u8; (w * h / 2) as usize];
        for p in uv.chunks_mut(2) {
            p[0] = u;
            p[1] = v;
        }
        VideoFrame::new_nv12(w, h, 0, vec![y; (w * h) as usize], uv, serial)
    }

    #[test]
    fn composites_layers() {
        let settings = CompositorSettings { canvas: (320, 180), background_rgb: [0, 0, 255], background_image: None, preview_width: 160, thumb_width: 64 };
        let mut c = match Compositor::new(&settings) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skipping GPU test: {e:#}");
                return;
            }
        };
        let a = c.add_source(64, 36);
        let b = c.add_source(64, 36);
        c.upload(a, &solid(64, 36, (1.0, 0.0, 0.0), 1));
        c.upload(b, &solid(64, 36, (0.0, 1.0, 0.0), 1));
        let states = [
            PadState { x: 0, y: 0, w: 160, h: 90, alpha: 1.0, zorder: 2 },
            PadState { x: 80, y: 45, w: 160, h: 90, alpha: 1.0, zorder: 3 },
        ];
        let r = c.render(&Layout::One(states.to_vec()), None, &[], 0, 0, &Wants { program: true, stream: Some((160, 90)), preview: true, preview_bus: false, thumbs: vec![0] }).unwrap();
        let p = r.program.unwrap();
        let px = |f: &VideoFrame, x: usize, y: usize| -> (u8, u8, u8) {
            f.pixels.with_planes(|pl| (pl.y[y * pl.y_stride + x], pl.uv[(y / 2) * pl.uv_stride + (x / 2) * 2], pl.uv[(y / 2) * pl.uv_stride + (x / 2) * 2 + 1]))
        };
        let near = |a: (u8, u8, u8), b: (u8, u8, u8)| (a.0 as i32 - b.0 as i32).abs() <= 3 && (a.1 as i32 - b.1 as i32).abs() <= 3 && (a.2 as i32 - b.2 as i32).abs() <= 3;
        let red = rgb_to_yuv(1.0, 0.0, 0.0);
        let green = rgb_to_yuv(0.0, 1.0, 0.0);
        let blue = rgb_to_yuv(0.0, 0.0, 1.0);
        assert!(near(px(&p, 20, 20), red), "{:?}", px(&p, 20, 20));
        assert!(near(px(&p, 120, 70), green), "green on top: {:?}", px(&p, 120, 70));
        assert!(near(px(&p, 300, 170), blue), "background: {:?}", px(&p, 300, 170));
        let s = r.stream.unwrap();
        assert_eq!((s.width, s.height), (160, 90));
        assert!(near(px(&s, 10, 10), red));
        let (pw, ph, prev) = r.preview.unwrap();
        assert_eq!((pw, ph, prev.len()), (160, 90, 160 * 90 * 4));
        assert!(prev[0] > 240 && prev[1] < 15, "preview top-left should be red");
        assert_eq!(r.thumbs.len(), 1);
        assert!(r.thumbs[0].3[0] > 240);
    }

    fn half_and_half(w: u32, h: u32, left: (f32, f32, f32), right: (f32, f32, f32)) -> VideoFrame {
        let (yl, ul, vl) = rgb_to_yuv(left.0, left.1, left.2);
        let (yr, ur, vr) = rgb_to_yuv(right.0, right.1, right.2);
        let mut y = vec![0u8; (w * h) as usize];
        let mut uv = vec![0u8; (w * h / 2) as usize];
        for r in 0..h as usize {
            for c in 0..w as usize {
                y[r * w as usize + c] = if c < w as usize / 2 { yl } else { yr };
            }
        }
        for r in 0..(h / 2) as usize {
            for c in 0..(w / 2) as usize {
                let (u, v) = if c < w as usize / 4 { (ul, vl) } else { (ur, vr) };
                uv[r * w as usize + c * 2] = u;
                uv[r * w as usize + c * 2 + 1] = v;
            }
        }
        VideoFrame::new_nv12(w, h, 0, y, uv, 1)
    }

    #[test]
    fn styles_keys_transitions_and_graphics() {
        let settings = CompositorSettings { canvas: (320, 180), background_rgb: [0, 0, 255], background_image: None, preview_width: 320, thumb_width: 64 };
        let mut c = match Compositor::new(&settings) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skipping GPU test: {e:#}");
                return;
            }
        };
        let a = c.add_source(64, 36);
        let b = c.add_source(64, 36);
        c.upload(a, &half_and_half(64, 36, (1.0, 0.0, 0.0), (1.0, 1.0, 0.0))); // red | yellow
        c.upload(b, &solid(64, 36, (0.0, 1.0, 0.0), 1)); // green
        let rgb = |r: &Rendered, x: usize, y: usize| -> [u8; 3] {
            let (w, _, d) = r.preview.as_ref().unwrap();
            let i = (y * *w as usize + x) * 4;
            [d[i], d[i + 1], d[i + 2]]
        };
        let near = |a: [u8; 3], b: [u8; 3]| a.iter().zip(b).all(|(x, y)| (*x as i32 - y as i32).abs() <= 12);
        let wants = Wants { preview: true, ..Default::default() };
        let full = PadState { x: 0, y: 0, w: 320, h: 180, alpha: 1.0, zorder: 2 };
        let hidden = PadState { alpha: 0.0, ..PadState::HIDDEN };
        let pip = PadState { x: 160, y: 90, w: 128, h: 72, alpha: 1.0, zorder: 3 };

        // Mirror: yellow on the left.
        let mut looks = vec![SourceLook { mirror: true, ..Default::default() }, SourceLook::default()];
        c.set_looks(&looks, &FrameStyle::default());
        let r = c.render(&Layout::One(vec![full, hidden]), None, &[], 0, 1, &wants).unwrap();
        assert!(near(rgb(&r, 40, 90), [255, 255, 0]), "mirrored: {:?}", rgb(&r, 40, 90));
        // Crop the right half away: all red, stretched to the frame.
        looks[0] = SourceLook { crop_right: 45.0, ..Default::default() };
        c.set_looks(&looks, &FrameStyle::default());
        let r = c.render(&Layout::One(vec![full, hidden]), None, &[], 0, 2, &wants).unwrap();
        assert!(near(rgb(&r, 250, 90), [255, 0, 0]), "cropped: {:?}", rgb(&r, 250, 90));
        // Saturation 0: grey.
        looks[0] = SourceLook { saturation: 0.0, ..Default::default() };
        c.set_looks(&looks, &FrameStyle::default());
        let r = c.render(&Layout::One(vec![full, hidden]), None, &[], 0, 3, &wants).unwrap();
        let g = rgb(&r, 40, 90);
        assert!((g[0] as i32 - g[1] as i32).abs() < 4 && (g[1] as i32 - g[2] as i32).abs() < 4, "grey: {g:?}");

        // Rounded corners, border and shadow on a picture-in-picture.
        looks[0] = SourceLook::default();
        let style = FrameStyle { corner_radius: 16.0, border_width: 4.0, border_color: "#ffffff".into(), shadow: 1.0 };
        c.set_looks(&looks, &style);
        let r = c.render(&Layout::One(vec![full, pip]), None, &[], 0, 4, &wants).unwrap();
        let corner = rgb(&r, 161, 91);
        assert!(!near(corner, [255, 255, 255]) && !near(corner, [0, 255, 0]) && corner[0] > 200, "corner is rounded off: {corner:?}");
        assert!(near(rgb(&r, 224, 91), [255, 255, 255]), "border: {:?}", rgb(&r, 224, 91));
        assert!(near(rgb(&r, 224, 120), [0, 255, 0]), "inside: {:?}", rgb(&r, 224, 120));
        // Chroma key on the green source: the picture below shows through.
        looks[1] = SourceLook { chroma_key: true, key_color: "#00ff00".into(), ..Default::default() };
        c.set_looks(&looks, &FrameStyle::default());
        let r = c.render(&Layout::One(vec![full, pip]), None, &[], 0, 5, &wants).unwrap();
        assert!(near(rgb(&r, 200, 120), [255, 0, 0]) || near(rgb(&r, 200, 120), [255, 255, 0]), "keyed: {:?}", rgb(&r, 200, 120));
        // Background removal: green full screen over red|yellow, with a
        // person mask covering only its right half.
        looks[1] = SourceLook { remove_background: true, ..Default::default() };
        c.set_looks(&looks, &FrameStyle::default());
        let full_top = PadState { zorder: 3, ..full };
        let r = c.render(&Layout::One(vec![full, full_top]), None, &[], 0, 50, &wants).unwrap();
        assert!(near(rgb(&r, 40, 90), [0, 255, 0]), "no mask yet: whole picture: {:?}", rgb(&r, 40, 90));
        let mask = crate::platform::vision::Mask { width: 16, height: 8, data: (0..128).map(|i| if i % 16 < 8 { 0 } else { 255 }).collect() };
        c.upload_mask(b, 7, &mask);
        let r = c.render(&Layout::One(vec![full, full_top]), None, &[], 0, 51, &wants).unwrap();
        assert!(near(rgb(&r, 40, 90), [255, 0, 0]), "background removed: {:?}", rgb(&r, 40, 90));
        assert!(near(rgb(&r, 280, 90), [0, 255, 0]), "person kept: {:?}", rgb(&r, 280, 90));
        c.clear_mask(b);
        let r = c.render(&Layout::One(vec![full, full_top]), None, &[], 0, 52, &wants).unwrap();
        assert!(near(rgb(&r, 40, 90), [0, 255, 0]), "mask cleared: {:?}", rgb(&r, 40, 90));

        // Wipe halfway, travelling right: the new scene (green) on the left.
        c.set_looks(&[SourceLook::default(), SourceLook::default()], &FrameStyle::default());
        let full_b = PadState { zorder: 2, ..full };
        let two = |mode, t| Layout::Two { from: vec![full, hidden], to: vec![hidden, full_b], mode, t, direction: [1.0, 0.0], dip: [0.0, 0.0, 0.0] };
        let r = c.render(&two(TransitionMode::Wipe, 0.5), None, &[], 0, 6, &wants).unwrap();
        assert!(near(rgb(&r, 40, 90), [0, 255, 0]) && near(rgb(&r, 300, 90), [255, 255, 0]), "wipe: {:?} {:?}", rgb(&r, 40, 90), rgb(&r, 300, 90));
        // Dip halfway: black.
        let r = c.render(&two(TransitionMode::Dip, 0.5), None, &[], 0, 7, &wants).unwrap();
        assert!(near(rgb(&r, 160, 90), [0, 0, 0]), "dip: {:?}", rgb(&r, 160, 90));
        // Push halfway (new scene comes in from the left): old red/yellow
        // picture's left half now fills the right half.
        let r = c.render(&two(TransitionMode::Push, 0.5), None, &[], 0, 8, &wants).unwrap();
        assert!(near(rgb(&r, 40, 90), [0, 255, 0]) && near(rgb(&r, 200, 90), [255, 0, 0]), "push: {:?} {:?}", rgb(&r, 40, 90), rgb(&r, 200, 90));

        // A graphic on top, and the preview bus showing another scene.
        let img = image::RgbaImage::from_pixel(32, 16, image::Rgba([255, 0, 255, 255]));
        let g = GraphicLayer { key: (0, 1), image: Arc::new(img), rect: [0.5, 0.5, 0.25, 0.25], alpha: 1.0, stream_only: false };
        let wants = Wants { preview: true, preview_bus: true, ..Default::default() };
        let r = c.render(&Layout::One(vec![full, hidden]), Some(&[hidden, full_b]), std::slice::from_ref(&g), 0, 9, &wants).unwrap();
        assert!(near(rgb(&r, 200, 110), [255, 0, 255]), "graphic: {:?}", rgb(&r, 200, 110));
        // A stream-only graphic (captions): on the stream, not the program.
        let green = image::RgbaImage::from_pixel(32, 16, image::Rgba([0, 255, 0, 255]));
        let cap = GraphicLayer { key: (9, 1), image: Arc::new(green), rect: [0.0, 0.0, 0.25, 0.25], alpha: 1.0, stream_only: true };
        let wants2 = Wants { program: true, stream: Some((160, 90)), ..Default::default() };
        let r2 = c.render(&Layout::One(vec![full, hidden]), None, std::slice::from_ref(&cap), 0, 10, &wants2).unwrap();
        let luma = |f: &VideoFrame, x: usize, y: usize| f.pixels.with_planes(|p| p.y[y * p.y_stride + x]);
        let (p, st) = (r2.program.unwrap(), r2.stream.unwrap());
        // Green (luma ~173) on the stream only; elsewhere they match.
        let (s1, p1) = (luma(&st, 10, 5) as i32, luma(&p, 20, 10) as i32);
        assert!((s1 - 173).abs() < 12 && (p1 - 173).abs() > 30, "stream {s1} program {p1}");
        let (s2, p2) = (luma(&st, 120, 60) as i32, luma(&p, 240, 120) as i32);
        assert!((s2 - p2).abs() < 10, "stream {s2} program {p2}");
        let (bw, _, bus) = r.preview_bus.as_ref().unwrap();
        let i = (20 * *bw as usize + 20) * 4;
        assert!(bus[i] < 20 && bus[i + 1] > 235, "preview bus shows green: {:?}", &bus[i..i + 3]);
        if let Ok(dir) = std::env::var("CUTAWAY_DUMP") {
            let (w, h, d) = r.preview.unwrap();
            image::RgbaImage::from_raw(w, h, d).unwrap().save(format!("{dir}/compositor.png")).unwrap();
        }
    }
}
