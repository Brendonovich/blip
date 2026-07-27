struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

struct CompositorSettings {
    content_rect: vec4<f32>,
    transform: vec4<f32>,
    canvas_data: vec4<f32>,
    color: vec4<f32>,
    shadow_data: vec4<f32>,
    shadow_color: vec4<f32>,
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
    let canvas_size = settings.canvas_data.xy;
    let item_size = canvas_size * settings.transform.zw;
    let half_size = item_size * 0.5;
    let shadow_offset = settings.shadow_data.xy;
    let shadow_margin = settings.shadow_data.z * 3.0;
    let shadow_half_size = max(half_size + vec2(settings.shadow_data.w), vec2(0.0));
    let left = min(-half_size.x, shadow_offset.x - shadow_half_size.x - shadow_margin);
    let right = max(half_size.x, shadow_offset.x + shadow_half_size.x + shadow_margin);
    let top = min(-half_size.y, shadow_offset.y - shadow_half_size.y - shadow_margin);
    let bottom = max(half_size.y, shadow_offset.y + shadow_half_size.y + shadow_margin);
    let expanded_size = vec2(right - left, bottom - top);
    let expanded_center = settings.transform.xy + vec2(
        (left + right) * 0.5 / canvas_size.x,
        (top + bottom) * 0.5 / canvas_size.y,
    );
    let scale = expanded_size / canvas_size;
    let offset = vec2(expanded_center.x * 2.0 - 1.0, 1.0 - expanded_center.y * 2.0);
    let unit_uv = uvs[vertex_index];
    let local_position = mix(vec2(left, top), vec2(right, bottom), unit_uv);
    return VertexOutput(
        vec4(positions[vertex_index] * scale + offset, 0.0, 1.0),
        local_position / item_size + vec2(0.5),
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

fn sample_nv12(
    center: vec2<f32>,
    full_range: bool,
    bt601: bool,
) -> vec3<f32> {
    let y = textureSample(frame, frame_sampler, center).r;
    let chroma = textureSample(frame_chroma, frame_sampler, center).rg;
    return nv12_to_rgb(y, chroma, full_range, bt601);
}

fn rounded_rect_distance(
    point: vec2<f32>,
    half_size: vec2<f32>,
    radius: f32,
    squircle: bool,
) -> f32 {
    let corner = abs(point) - (half_size - vec2(radius));
    let outside = max(corner, vec2(0.0));
    let circular_distance = length(outside);
    let squircle_distance = pow(pow(outside.x, 4.0) + pow(outside.y, 4.0), 0.25);
    let curved_distance = select(circular_distance, squircle_distance, squircle);
    return curved_distance + min(max(corner.x, corner.y), 0.0) - radius;
}

fn erf_approx(value: vec2<f32>) -> vec2<f32> {
    let value_sign = sign(value);
    let absolute = abs(value);
    let polynomial = 1.0 + (
        0.278393 + (0.230389 + (0.000972 + 0.078108 * absolute) * absolute) * absolute
    ) * absolute;
    let squared = polynomial * polynomial;
    return value_sign - value_sign / (squared * squared);
}

fn gaussian(value: f32, sigma: f32) -> f32 {
    return exp(-(value * value) / (2.0 * sigma * sigma)) / (sqrt(2.0 * 3.14159265) * sigma);
}

fn blur_along_x(
    x: f32,
    y: f32,
    sigma: f32,
    corner_radius: f32,
    half_size: vec2<f32>,
) -> f32 {
    let delta = min(half_size.y - corner_radius - abs(y), 0.0);
    let curved = half_size.x - corner_radius + sqrt(max(
        0.0,
        corner_radius * corner_radius - delta * delta,
    ));
    let integral = 0.5 + 0.5 * erf_approx(
        (vec2(x - curved, x + curved)) * (sqrt(0.5) / sigma),
    );
    return integral.y - integral.x;
}

fn shadow_coverage(
    point: vec2<f32>,
    half_size: vec2<f32>,
    corner_radius: f32,
    blur_radius: f32,
) -> f32 {
    if blur_radius <= 0.0 {
        return clamp(0.5 - rounded_rect_distance(point, half_size, corner_radius, false), 0.0, 1.0);
    }

    let low = point.y - half_size.y;
    let high = point.y + half_size.y;
    let start = clamp(-3.0 * blur_radius, low, high);
    let end = clamp(3.0 * blur_radius, low, high);
    let sample_step = (end - start) / 4.0;
    var y = start + sample_step * 0.5;
    var coverage = 0.0;
    for (var index = 0; index < 4; index += 1) {
        coverage += blur_along_x(
            point.x,
            point.y - y,
            blur_radius,
            corner_radius,
            half_size,
        ) * gaussian(y, blur_radius) * sample_step;
        y += sample_step;
    }
    return coverage;
}

fn source_over(foreground: vec4<f32>, background: vec4<f32>) -> vec4<f32> {
    let alpha = foreground.a + background.a * (1.0 - foreground.a);
    if alpha <= 0.0001 {
        return vec4(0.0);
    }
    let rgb = (
        foreground.rgb * foreground.a +
        background.rgb * background.a * (1.0 - foreground.a)
    ) / alpha;
    return vec4(rgb, alpha);
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let texture_dimensions = vec2<f32>(textureDimensions(frame));
    let source_uv = (
        settings.content_rect.xy + input.uv * settings.content_rect.zw
    ) / texture_dimensions;
    let content_kind = settings.canvas_data.w;
    var color = textureSample(frame, frame_sampler, source_uv);
    if content_kind > 0.5 && content_kind < 1.5 {
        color = settings.color;
    } else if content_kind == 6.0 {
        let progress = clamp((input.uv.x + input.uv.y) * 0.5, 0.0, 1.0);
        color = mix(settings.color, settings.content_rect, progress);
    } else if content_kind == 7.0 {
        let cell = vec2<i32>(floor(input.uv * vec2(12.0, 8.0)));
        color = select(settings.color, settings.content_rect, (cell.x + cell.y) % 2 == 0);
    } else if content_kind > 1.5 {
        let full_range = content_kind == 3.0 || content_kind == 5.0;
        color = vec4(sample_nv12(
            source_uv,
            full_range,
            content_kind > 3.5,
        ), 1.0);
    }
    let dimensions = settings.canvas_data.xy * settings.transform.zw;
    let point = (input.uv - vec2(0.5)) * dimensions;
    let encoded_radius = settings.canvas_data.z;
    let radius = abs(encoded_radius);
    let distance = rounded_rect_distance(
        point,
        dimensions * 0.5,
        radius,
        encoded_radius < 0.0,
    );
    let antialias = max(fwidth(distance), 0.0001);
    color.a *= 1.0 - smoothstep(-antialias, antialias, distance);

    let shadow_half_size = max(
        dimensions * 0.5 + vec2(settings.shadow_data.w),
        vec2(0.0),
    );
    let shadow_radius = clamp(
        radius + settings.shadow_data.w,
        0.0,
        min(shadow_half_size.x, shadow_half_size.y),
    );
    var shadow_color = settings.shadow_color;
    shadow_color.a *= shadow_coverage(
        point - settings.shadow_data.xy,
        shadow_half_size,
        shadow_radius,
        settings.shadow_data.z,
    );
    return source_over(color, shadow_color);
}
