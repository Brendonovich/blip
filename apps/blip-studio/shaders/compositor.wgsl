struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

struct CompositorSettings {
    content_rect: vec4<f32>,
    transform: vec4<f32>,
    canvas_data: vec4<f32>,
    color: vec4<f32>,
};

@group(0) @binding(2) var<uniform> settings: CompositorSettings;

@vertex
fn vertex(
    @builtin(vertex_index) vertex_index: u32,
) -> VertexOutput {
    let positions = array(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0),
        vec2(-1.0, 1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
    );
    let uvs = array(
        vec2(0.0, 1.0), vec2(1.0, 1.0), vec2(0.0, 0.0),
        vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(1.0, 0.0),
    );
    let scale = settings.transform.zw;
    let offset = vec2(
        settings.transform.x * 2.0 - 1.0,
        1.0 - settings.transform.y * 2.0,
    );
    return VertexOutput(
        vec4(positions[vertex_index] * scale + offset, 0.0, 1.0),
        uvs[vertex_index],
    );
}

@group(0) @binding(0) var frame: texture_2d<f32>;
@group(0) @binding(1) var frame_sampler: sampler;
@group(0) @binding(3) var frame_chroma: texture_2d<f32>;

fn nv12_to_rgb(y_sample: f32, chroma: vec2<f32>, full_range: bool, bt601: bool) -> vec3<f32> {
    let y = select((y_sample - 16.0 / 255.0) * (255.0 / 219.0), y_sample, full_range);
    let chroma_scale = select(255.0 / 224.0, 1.0, full_range);
    let cb = (chroma.x - 128.0 / 255.0) * chroma_scale;
    let cr = (chroma.y - 128.0 / 255.0) * chroma_scale;
    let bt709_color = vec3(
        y + 1.5748 * cr,
        y - 0.1873 * cb - 0.4681 * cr,
        y + 1.8556 * cb,
    );
    let bt601_color = vec3(
        y + 1.402 * cr,
        y - 0.344136 * cb - 0.714136 * cr,
        y + 1.772 * cb,
    );
    return clamp(select(bt709_color, bt601_color, bt601), vec3(0.0), vec3(1.0));
}

fn sample_uv(
    center: vec2<f32>,
    offset: vec2<f32>,
    texture_dimensions: vec2<f32>,
    content_min: vec2<f32>,
    content_max: vec2<f32>,
) -> vec2<f32> {
    let half_texel = 0.5 / texture_dimensions;
    return clamp(center + offset / texture_dimensions, content_min + half_texel, content_max - half_texel);
}

fn sample_bgra(
    center: vec2<f32>,
    footprint: vec2<f32>,
    texture_dimensions: vec2<f32>,
    content_min: vec2<f32>,
    content_max: vec2<f32>,
) -> vec4<f32> {
    if max(footprint.x, footprint.y) <= 1.1 {
        return textureSample(frame, frame_sampler, center);
    }

    let offsets = array(-0.375, -0.125, 0.125, 0.375);
    let span = select(vec2(0.0), footprint, footprint > vec2(1.1));
    var color = vec4(0.0);
    for (var y = 0u; y < 4u; y += 1u) {
        for (var x = 0u; x < 4u; x += 1u) {
            let uv = sample_uv(
                center,
                vec2(offsets[x] * span.x, offsets[y] * span.y),
                texture_dimensions,
                content_min,
                content_max,
            );
            color += textureSample(frame, frame_sampler, uv);
        }
    }
    return color * (1.0 / 16.0);
}

fn sample_nv12(
    center: vec2<f32>,
    footprint: vec2<f32>,
    texture_dimensions: vec2<f32>,
    content_min: vec2<f32>,
    content_max: vec2<f32>,
    full_range: bool,
    bt601: bool,
) -> vec3<f32> {
    if max(footprint.x, footprint.y) <= 1.1 {
        let y = textureSample(frame, frame_sampler, center).r;
        let chroma = textureSample(frame_chroma, frame_sampler, center).rg;
        return nv12_to_rgb(y, chroma, full_range, bt601);
    }

    let offsets = array(-0.375, -0.125, 0.125, 0.375);
    let span = select(vec2(0.0), footprint, footprint > vec2(1.1));
    var y_sample = 0.0;
    var chroma = vec2(0.0);
    for (var y = 0u; y < 4u; y += 1u) {
        for (var x = 0u; x < 4u; x += 1u) {
            let uv = sample_uv(
                center,
                vec2(offsets[x] * span.x, offsets[y] * span.y),
                texture_dimensions,
                content_min,
                content_max,
            );
            y_sample += textureSample(frame, frame_sampler, uv).r;
            chroma += textureSample(frame_chroma, frame_sampler, uv).rg;
        }
    }
    return nv12_to_rgb(
        y_sample * (1.0 / 16.0),
        chroma * (1.0 / 16.0),
        full_range,
        bt601,
    );
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let texture_dimensions = vec2<f32>(textureDimensions(frame));
    let source_uv = (
        settings.content_rect.xy + input.uv * settings.content_rect.zw
    ) / texture_dimensions;
    let content_min = settings.content_rect.xy / texture_dimensions;
    let content_max = (settings.content_rect.xy + settings.content_rect.zw) / texture_dimensions;
    let footprint = fwidth(source_uv) * texture_dimensions;
    let content_kind = settings.canvas_data.w;
    var color = sample_bgra(
        source_uv,
        footprint,
        texture_dimensions,
        content_min,
        content_max,
    );
    if content_kind > 0.5 && content_kind < 1.5 {
        color = settings.color;
    } else if content_kind > 1.5 {
        let full_range = content_kind == 3.0 || content_kind == 5.0;
        color = vec4(sample_nv12(
            source_uv,
            footprint,
            texture_dimensions,
            content_min,
            content_max,
            full_range,
            content_kind > 3.5,
        ), 1.0);
    }
    let radius = settings.canvas_data.z;
    if radius > 0.0 {
        let dimensions = settings.canvas_data.xy * settings.transform.zw;
        let centered = abs((input.uv - vec2(0.5)) * dimensions);
        let corner = centered - (dimensions * 0.5 - vec2(radius));
        let distance = length(max(corner, vec2(0.0))) + min(max(corner.x, corner.y), 0.0) - radius;
        let antialias = fwidth(distance);
        if distance > antialias {
            discard;
        }
        color.a *= 1.0 - smoothstep(-antialias, antialias, distance);
    }
    return color;
}
