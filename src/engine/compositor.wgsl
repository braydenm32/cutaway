// Cutaway compositor shaders.
//
// Every draw is one quad (triangle strip, 4 vertices) placed by `u.rect` in
// clip space. Full-screen passes sample `u.uv`; source layers work out
// their own texture coordinates from the pixel position (`u.px`), so the
// quad can extend past the picture for borders and shadows.

struct U {
    rect: vec4<f32>,   // x0, y0, x1, y1 (clip space; y0 = top)
    uv: vec4<f32>,     // u0, v0, u1, v1 (u0 > u1 mirrors)
    params: vec4<f32>, // x: alpha, y: taps per axis, z/w: footprint of one output pixel in uv units
    px: vec4<f32>,     // layer picture rect in target pixels: x0, y0, x1, y1
    style: vec4<f32>,  // corner radius, border width, shadow strength, style amount (0..1)
    style2: vec4<f32>, // shadow offset x, y, shadow blur, unused
    border: vec4<f32>, // border rgb, unused
    color: vec4<f32>,  // brightness, contrast, saturation, unused
    key: vec4<f32>,    // key colour cb, cr, similarity, smoothness
    key2: vec4<f32>,   // spill, key enabled (0/1), person mask enabled (0/1), unused
    keyrgb: vec4<f32>, // key colour rgb, unused
    trans: vec4<f32>,  // kind (2 wipe, 3 push, 4 dip), progress 0..1, direction x, y
    dip: vec4<f32>,    // dip colour rgb, unused
    lut: vec4<f32>,    // strength (0 = none), domain min, domain max, points
    bg: vec4<f32>,     // behind the people: mode (0 cut out, 1 blur, 2 picture), blur radius (of width), picture scale x, y
};

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var tex_a: texture_2d<f32>;
@group(0) @binding(3) var tex_b: texture_2d<f32>;
@group(0) @binding(4) var tex_mask: texture_2d<f32>;
@group(0) @binding(5) var tex_lut: texture_3d<f32>;
@group(0) @binding(6) var tex_bg: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    var corners = array<vec2<f32>, 4>(vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0));
    let c = corners[i];
    var o: VsOut;
    o.pos = vec4<f32>(mix(u.rect.x, u.rect.z, c.x), mix(u.rect.y, u.rect.w, c.y), 0.0, 1.0);
    o.uv = vec2<f32>(mix(u.uv.x, u.uv.z, c.x), mix(u.uv.y, u.uv.w, c.y));
    return o;
}

// BT.709 video range -> RGB.
fn yuv_to_rgb(y: f32, cb: f32, cr: f32) -> vec3<f32> {
    let yy = (y * 255.0 - 16.0) / 219.0;
    let cu = (cb * 255.0 - 128.0) / 224.0;
    let cv = (cr * 255.0 - 128.0) / 224.0;
    return clamp(vec3<f32>(yy + 1.5748 * cv, yy - 0.1873 * cu - 0.4681 * cv, yy + 1.8556 * cu), vec3<f32>(0.0), vec3<f32>(1.0));
}

// A plain source picture: tex_a = Y plane (R8), tex_b = CbCr plane (RG8).
// Used for the multiview thumbnails.
@fragment
fn fs_yuv(in: VsOut) -> @location(0) vec4<f32> {
    let y = textureSample(tex_a, samp, in.uv).r;
    let c = textureSample(tex_b, samp, in.uv).rg;
    return vec4<f32>(yuv_to_rgb(y, c.r, c.g), u.params.x);
}

// Signed distance to a rounded rectangle centred on the origin.
fn sd_round_box(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - half + vec2<f32>(r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn apply_lut(c: vec3<f32>) -> vec3<f32> {
    if (u.lut.x <= 0.0) {
        return c;
    }
    let n = u.lut.w;
    let p = clamp((c - vec3<f32>(u.lut.y)) / max(u.lut.z - u.lut.y, 1e-5), vec3<f32>(0.0), vec3<f32>(1.0));
    let graded = textureSampleLevel(tex_lut, samp, p * ((n - 1.0) / n) + vec3<f32>(0.5 / n), 0.0).rgb;
    return mix(c, clamp(graded, vec3<f32>(0.0), vec3<f32>(1.0)), u.lut.x);
}

// The source's own background, blurred like a portrait lens: a spiral of
// taps over a disc, leaving out the person (so they don't glow around the
// edges).
fn blurred_background(uv: vec2<f32>) -> vec3<f32> {
    let dims = vec2<f32>(textureDimensions(tex_a));
    let r = vec2<f32>(u.bg.y, u.bg.y * dims.x / max(dims.y, 1.0));
    var acc = vec3<f32>(0.0);
    var every = vec3<f32>(0.0);
    var w = 0.0;
    let taps = 40;
    for (var i = 0; i < taps; i = i + 1) {
        let fi = f32(i) + 0.5;
        let k = sqrt(fi / f32(taps));
        let a = fi * 2.39996;
        let q = uv + vec2<f32>(cos(a), sin(a)) * k * r;
        let y = textureSampleLevel(tex_a, samp, q, 0.0).r;
        let c = textureSampleLevel(tex_b, samp, q, 0.0).rg;
        let rgb = yuv_to_rgb(y, c.r, c.g);
        let bw = 1.0 - textureSampleLevel(tex_mask, samp, q, 0.0).r;
        acc = acc + rgb * bw;
        every = every + rgb;
        w = w + bw;
    }
    // Nearly all person around here: the plain average will do.
    let b = mix(every / f32(taps), acc / max(w, 1e-4), smoothstep(0.5, 3.0, w));
    return apply_lut(b);
}

// A source on the program: crop/mirror (via uv), colour, chroma key or
// person mask, rounded corners, border and drop shadow.
@fragment
fn fs_layer(in: VsOut) -> @location(0) vec4<f32> {
    let p = in.pos.xy;
    let size = max(u.px.zw - u.px.xy, vec2<f32>(1.0));
    let centre = (u.px.xy + u.px.zw) * 0.5;
    let amount = u.style.w;
    let radius = min(u.style.x * amount, min(size.x, size.y) * 0.5);
    let bw = u.style.y * amount;
    let d = sd_round_box(p - centre, size * 0.5, radius);

    // Picture.
    let t = (p - u.px.xy) / size;
    let uv = mix(u.uv.xy, u.uv.zw, t);
    let yy = textureSampleLevel(tex_a, samp, uv, 0.0).r;
    let cc = textureSampleLevel(tex_b, samp, uv, 0.0).rg;
    var rgb = yuv_to_rgb(yy, cc.r, cc.g);
    // Colour LUT (a camera's log to Rec.709, or a look), sampled at the
    // table's points.
    rgb = apply_lut(rgb);
    var key_alpha = 1.0;
    if (u.key2.y > 0.5) {
        let dist = distance(cc, u.key.xy);
        key_alpha = smoothstep(u.key.z, u.key.z + max(u.key.w, 0.001), dist);
        // Spill: remove the key colour's cast from the edges.
        let avg = (rgb.r + rgb.g + rgb.b) / 3.0;
        let kd = u.keyrgb.rgb - vec3<f32>((u.keyrgb.r + u.keyrgb.g + u.keyrgb.b) / 3.0);
        let kl = length(kd);
        if (kl > 0.01) {
            let k = kd / kl;
            let s = max(dot(rgb - vec3<f32>(avg), k), 0.0);
            rgb = rgb - k * s * u.key2.x;
        }
    }
    // Background removal: the person mask (soft at the edges). Cut out,
    // blurred, or (after the colour, below) swapped for a picture.
    let mode = i32(u.bg.x + 0.5);
    var person = 1.0;
    if (u.key2.z > 0.5) {
        person = smoothstep(0.15, 0.85, textureSampleLevel(tex_mask, samp, uv, 0.0).r);
        if (mode == 0) {
            key_alpha = key_alpha * person;
        } else if (mode == 1 && person < 0.999) {
            rgb = mix(blurred_background(uv), rgb, person);
        }
    }
    // Colour: brightness, contrast, saturation.
    rgb = (rgb - vec3<f32>(0.5)) * u.color.y + vec3<f32>(0.5) + vec3<f32>(u.color.x);
    let luma = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    rgb = clamp(mix(vec3<f32>(luma), rgb, u.color.z), vec3<f32>(0.0), vec3<f32>(1.0));
    if (u.key2.z > 0.5 && mode == 2 && person < 0.999) {
        let back = textureSampleLevel(tex_bg, samp, (t - vec2<f32>(0.5)) * u.bg.zw + vec2<f32>(0.5), 0.0).rgb;
        rgb = mix(back, rgb, person);
    }

    let inside = 1.0 - smoothstep(-0.5, 0.5, d);
    let in_pic = 1.0 - smoothstep(-0.5, 0.5, d + bw);
    let col = mix(u.border.rgb, rgb, in_pic);
    let a = inside * mix(1.0, key_alpha, in_pic);

    // Drop shadow under the frame.
    var sh_a = 0.0;
    let sh = u.style.z * amount;
    if (sh > 0.0) {
        let blur = max(u.style2.z, 1.0);
        let ds = sd_round_box(p - centre - u.style2.xy, size * 0.5, radius);
        sh_a = sh * 0.65 * (1.0 - smoothstep(-blur * 0.5, blur, ds));
    }
    let out_a = a + sh_a * (1.0 - a);
    let out_rgb = col * a / max(out_a, 1e-5);
    return vec4<f32>(out_rgb, out_a * u.params.x);
}

// An RGBA picture (background image, graphics, previews), averaged over the
// output pixel's footprint so big reductions don't shimmer.
fn sample_area(uv: vec2<f32>) -> vec4<f32> {
    let taps = i32(u.params.y);
    if (taps <= 1) {
        return textureSampleLevel(tex_a, samp, uv, 0.0);
    }
    var acc = vec4<f32>(0.0);
    let step = u.params.zw / f32(taps);
    let origin = uv - u.params.zw * 0.5 + step * 0.5;
    for (var j = 0; j < taps; j = j + 1) {
        for (var i = 0; i < taps; i = i + 1) {
            acc = acc + textureSampleLevel(tex_a, samp, origin + step * vec2<f32>(f32(i), f32(j)), 0.0);
        }
    }
    return acc / f32(taps * taps);
}

@fragment
fn fs_rgba(in: VsOut) -> @location(0) vec4<f32> {
    let c = sample_area(in.uv);
    return vec4<f32>(c.rgb, c.a * u.params.x);
}

// Two whole scenes: tex_a = outgoing, tex_b = incoming.
@fragment
fn fs_trans(in: VsOut) -> @location(0) vec4<f32> {
    let e = u.trans.y;
    let dir = u.trans.zw;
    let kind = i32(u.trans.x + 0.5);
    let uv = in.uv;
    if (kind == 2) {
        // Wipe: a soft edge moves across in `dir`.
        var s = 0.0;
        if (dir.x > 0.5) { s = uv.x; } else if (dir.x < -0.5) { s = 1.0 - uv.x; } else if (dir.y > 0.5) { s = uv.y; } else { s = 1.0 - uv.y; }
        let edge = 0.02;
        let m = 1.0 - smoothstep(e * (1.0 + edge) - edge, e * (1.0 + edge), s);
        let a = textureSampleLevel(tex_a, samp, uv, 0.0);
        let b = textureSampleLevel(tex_b, samp, uv, 0.0);
        return vec4<f32>(mix(a.rgb, b.rgb, m), 1.0);
    }
    if (kind == 3) {
        // Push: the new scene slides in and pushes the old one out.
        let shifted = uv - dir * e;
        let inside_a = shifted.x >= 0.0 && shifted.x <= 1.0 && shifted.y >= 0.0 && shifted.y <= 1.0;
        if (inside_a) {
            return vec4<f32>(textureSampleLevel(tex_a, samp, shifted, 0.0).rgb, 1.0);
        }
        return vec4<f32>(textureSampleLevel(tex_b, samp, shifted + dir, 0.0).rgb, 1.0);
    }
    // Dip: fade to a colour, then up on the new scene.
    let a = textureSampleLevel(tex_a, samp, uv, 0.0).rgb;
    let b = textureSampleLevel(tex_b, samp, uv, 0.0).rgb;
    if (e < 0.5) {
        return vec4<f32>(mix(a, u.dip.rgb, e * 2.0), 1.0);
    }
    return vec4<f32>(mix(u.dip.rgb, b, (e - 0.5) * 2.0), 1.0);
}

// A stinger over the program: tex_a/tex_b = its Y/CbCr planes; u.uv =
// where the picture is; u.key.x = 0 solid, 1 matte at uv + u.key.yz,
// 2 black is see-through.
@fragment
fn fs_stinger(in: VsOut) -> @location(0) vec4<f32> {
    let y = textureSampleLevel(tex_a, samp, in.uv, 0.0).r;
    let c = textureSampleLevel(tex_b, samp, in.uv, 0.0).rg;
    let rgb = yuv_to_rgb(y, c.r, c.g);
    let kind = i32(u.key.x + 0.5);
    if (kind == 1) {
        let m = textureSampleLevel(tex_a, samp, in.uv + u.key.yz, 0.0).r;
        return vec4<f32>(rgb, clamp((m * 255.0 - 16.0) / 219.0, 0.0, 1.0));
    }
    if (kind == 2) {
        // Light on black: adds over the program.
        let a = max(rgb.r, max(rgb.g, rgb.b));
        return vec4<f32>(rgb / max(a, 1e-4), a);
    }
    return vec4<f32>(rgb, 1.0);
}

// Canvas -> Y plane.
@fragment
fn fs_out_y(in: VsOut) -> @location(0) vec4<f32> {
    let c = sample_area(in.uv).rgb;
    let y = (16.0 + 219.0 * dot(c, vec3<f32>(0.2126, 0.7152, 0.0722))) / 255.0;
    return vec4<f32>(y, 0.0, 0.0, 1.0);
}

// Canvas -> CbCr plane (half size).
@fragment
fn fs_out_uv(in: VsOut) -> @location(0) vec4<f32> {
    let c = sample_area(in.uv).rgb;
    let cb = (128.0 + 224.0 * dot(c, vec3<f32>(-0.1146, -0.3854, 0.5))) / 255.0;
    let cr = (128.0 + 224.0 * dot(c, vec3<f32>(0.5, -0.4542, -0.0458))) / 255.0;
    return vec4<f32>(cb, cr, 0.0, 1.0);
}
