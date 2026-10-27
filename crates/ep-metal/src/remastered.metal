// electropaint Remastered: ep's strand as polished, iridescent metal tiles
// in a studio of soft lights, in high dynamic range.
//
// Passes: the smearing strands (tiles or ribbons, 4x MSAA) into a layer
// with transparent background, and that over their decayed trail; the other
// strands into the layer, with the trail merged under them; a bloom chain
// from the bright parts; and the final picture: a dark backdrop, the
// strands, the bloom, tone mapped to the display's headroom.

#include <metal_stdlib>
using namespace metal;

// ------------------------------------------------------------------ types

struct View {
    float4x4 proj;
    float4 background; // ep's background colour, linear
};

struct MeshVertex {
    float4 pos;    // model space; w = how near the rim (0 centre, 1 edge)
    float4 normal;
};

struct Instance {
    float4x4 model_view;
    float4 fill;    // linear rgb, alpha
    float4 outline;
    float hue;
    uint flags;     // FILL | OUTLINE | FAT
    float pad[2];
};

struct RibbonVertex {
    float4 pos;     // eye space
    float4 normal;
    float4 colour;  // linear
};

constant uint FILL = 1;
constant uint OUTLINE = 2;
constant uint FAT = 4;

struct Shaded {
    float4 position [[position]];
    float3 eye;
    float3 normal;
    float edge;
    float4 fill [[flat]];
    float4 outline [[flat]];
    float hue [[flat]];
    uint flags [[flat]];
};

struct Post {
    float4 background;
    float2 texel;      // 1 / size of the texture read
    float decay;       // trail kept per step
    float threshold;   // bloom
    float headroom;    // brightest value the display shows (1 = SDR white)
    float tan_half_fov;
    float aspect;
    float bloom;
    uint srgb;         // encode for an 8-bit target
    uint pad[3];
};

constexpr sampler linear_clamp(filter::linear, address::clamp_to_edge);

// ------------------------------------------------------------ environment

// A soft-edged rectangular light centred on direction `c`.
static float softbox(float3 d, float3 c, float2 half_size, float soft) {
    float k = dot(d, c);
    if (k <= 0.0) {
        return 0.0;
    }
    float3 t = normalize(cross(abs(c.y) < 0.99 ? float3(0, 1, 0) : float3(1, 0, 0), c));
    float3 b = cross(c, t);
    float2 p = abs(float2(dot(d, t), dot(d, b)) / k);
    float2 q = smoothstep(half_size + soft, half_size - soft, p);
    return q.x * q.y;
}

// The backdrop behind the strand: near black, a little lighter towards the
// top, tinted by ep's background colour when it has one.
static float3 backdrop(float3 d, float3 tint) {
    float3 c = mix(float3(0.0004, 0.0004, 0.0006), float3(0.0025, 0.0027, 0.0040),
                   smoothstep(-0.6, 0.9, d.y));
    return c + tint * 0.25 * smoothstep(-0.5, 1.0, d.y);
}

// The studio the tiles reflect, in eye space: a dim dome all round with a
// bright horizon, and soft lights on every side. (The camera sees only the
// backdrop: the studio is what the metal shows.) Rougher reflections see
// softer lights.
static float3 studio(float3 d, float3 tint, float rough) {
    float s = 0.03 + rough * 0.6;
    float3 c = backdrop(d, tint);
    c += mix(0.02, 0.22, smoothstep(-0.8, 0.9, d.y)) * (float3(0.92, 0.95, 1.0) + tint);
    c += 0.6 * float3(0.85, 0.9, 1.0) * exp(-abs(d.y + 0.05) * 18.0 / (1.0 + rough * 8.0));
    c += 3.0 * float3(1.00, 0.98, 0.96) * softbox(d, normalize(float3(0.0, 1.0, 0.1)), float2(0.60, 0.60), s);
    c += 6.0 * float3(1.00, 0.98, 0.95) * softbox(d, normalize(float3(0.0, 0.28, 1.0)), float2(1.10, 0.035), s);
    c += 8.0 * float3(1.00, 0.97, 0.92) * softbox(d, normalize(float3(-0.35, 0.55, 0.75)), float2(0.45, 0.28), s);
    c += 4.5 * float3(0.85, 0.92, 1.00) * softbox(d, normalize(float3(0.95, 0.05, 0.30)), float2(0.07, 0.90), s);
    c += 3.0 * float3(1.00, 0.88, 0.80) * softbox(d, normalize(float3(-0.95, -0.10, 0.25)), float2(0.07, 0.90), s);
    c += 3.0 * float3(1.00, 0.85, 0.70) * softbox(d, normalize(float3(-0.60, 0.20, -0.75)), float2(0.25, 0.50), s);
    c += 3.0 * float3(0.70, 0.85, 1.00) * softbox(d, normalize(float3(0.70, -0.10, -0.70)), float2(0.06, 0.80), s);
    c += 1.6 * float3(0.30, 0.60, 1.00) * softbox(d, normalize(float3(0.40, -0.70, 0.60)), float2(0.50, 0.15), s);
    c += 1.4 * float3(1.00, 0.35, 0.60) * softbox(d, normalize(float3(0.00, 0.90, -0.40)), float2(0.60, 0.20), s);
    return c;
}

// --------------------------------------------------------------- shading

// Interference in a thin transparent film over the metal: how strongly
// each of red, green and blue is reflected, for a film `nm` thick seen at
// `cos_i` to its normal.
static float3 thin_film(float cos_i, float nm) {
    const float n = 1.45;
    float cos_t = sqrt(1.0 - (1.0 - cos_i * cos_i) / (n * n));
    float path = 2.0 * n * nm * cos_t;
    return 0.5 + 0.5 * cos(2.0 * M_PI_F * path / float3(650.0, 540.0, 450.0));
}

static float ggx(float n_h, float a) {
    float a2 = a * a;
    float d = n_h * n_h * (a2 - 1.0) + 1.0;
    return a2 / (M_PI_F * d * d);
}

static float smith(float n_v, float n_l, float a) {
    float k = a * 0.5;
    return 0.25 / ((n_v * (1.0 - k) + k) * (n_l * (1.0 - k) + k));
}

static Shaded shaded(float4 eye, float3 normal, float edge, float4 fill, float4 outline,
                     float hue, uint flags, constant View &view) {
    Shaded o;
    o.position = view.proj * eye;
    o.eye = eye.xyz;
    o.normal = normal;
    o.edge = edge;
    o.fill = fill;
    o.outline = outline;
    o.hue = hue;
    o.flags = flags;
    return o;
}

vertex Shaded tile_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                          const device MeshVertex *mesh [[buffer(0)]],
                          const device Instance *instances [[buffer(1)]],
                          constant View &view [[buffer(2)]]) {
    MeshVertex v = mesh[vid];
    Instance t = instances[iid];
    float4 eye = t.model_view * float4(v.pos.xyz, 1.0);
    float3 c0 = t.model_view[0].xyz, c1 = t.model_view[1].xyz, c2 = t.model_view[2].xyz;
    // The cofactor matrix (det times the inverse transpose) carries normals
    // through any scale; a mirrored copy (det < 0) needs them flipped back.
    float3x3 cof = float3x3(cross(c1, c2), cross(c2, c0), cross(c0, c1));
    float det = dot(c0, cross(c1, c2));
    float3 n = cof * v.normal.xyz * (det < 0.0 ? -1.0 : 1.0);
    return shaded(eye, n, v.pos.w, t.fill, t.outline, t.hue, t.flags, view);
}

vertex Shaded ribbon_vertex(uint vid [[vertex_id]],
                            const device RibbonVertex *verts [[buffer(0)]],
                            constant View &view [[buffer(2)]]) {
    RibbonVertex v = verts[vid];
    return shaded(float4(v.pos.xyz, 1.0), v.normal.xyz, 0.0, v.colour, float4(0.0),
                  v.normal.w, FILL, view);
}

fragment float4 shade_fragment(Shaded in [[stage_in]], constant View &view [[buffer(0)]]) {
    bool fill = (in.flags & FILL) != 0;
    bool outline = (in.flags & OUTLINE) != 0;
    bool fat = (in.flags & FAT) != 0;
    float rim_from = fat ? 0.80 : 0.92;
    float rim = outline ? smoothstep(rim_from, rim_from + 0.05, in.edge) : 0.0;
    if (!fill && rim < 0.02) {
        discard_fragment();
    }
    // Over a fill the outline is a fine glowing inlay; alone it is the tile.
    float3 glow = in.outline.rgb * in.outline.a * rim * (fill ? (fat ? 1.6 : 0.9) : (fat ? 5.0 : 3.0));
    if (!fill) {
        return float4(glow, 1.0);
    }

    float3 v = normalize(-in.eye);
    float3 n = normalize(in.normal);
    if (dot(n, v) < 0.0) {
        n = -n; // the back of a ribbon, or a tile seen edge-on
    }
    // Clamped above too: rounding can take the dot product just past 1, and
    // pow of the negative 1 - n_v below is NaN, which the trail would keep.
    float n_v = clamp(dot(n, v), 1e-3, 1.0);
    const float rough = 0.10;
    float a = rough * rough;

    // Coloured chrome under an iridescent film whose thickness follows the
    // slot's hue, so ep's colour controls still colour the strand.
    float3 base = mix(in.fill.rgb, float3(0.97), 0.20);
    float3 film = thin_film(n_v, 260.0 + 520.0 * in.hue);
    float3 f0 = clamp(base * mix(float3(1.0), 0.30 + 1.10 * film, 0.65), 0.02, 1.0);
    float3 fresnel = f0 + (1.0 - f0) * pow(1.0 - n_v, 5.0);

    float3 r = reflect(-v, n);
    float3 colour = studio(r, view.background.rgb, rough) * fresnel;

    // ep's own two lights, fixed in eye space, as sharp highlights.
    const float3 lights[2] = {normalize(float3(0.0, 1.0, 0.5)), normalize(float3(0.0, -1.0, -0.5))};
    const float3 power[2] = {float3(7.0, 6.8, 6.4), float3(2.5, 3.0, 4.0)};
    for (int i = 0; i < 2; i++) {
        float3 l = lights[i];
        float n_l = dot(n, l);
        if (n_l > 0.0) {
            float3 h = normalize(l + v);
            colour += power[i] * fresnel * ggx(max(dot(n, h), 0.0), a) * smith(n_v, n_l, a) * n_l;
        }
    }
    return float4(colour + glow, 1.0);
}

// ---------------------------------------------------------- post passes

struct Screen {
    float4 position [[position]];
    float2 uv;
};

vertex Screen screen_vertex(uint vid [[vertex_id]]) {
    float2 p = float2((vid << 1) & 2, vid & 2);
    Screen o;
    o.position = float4(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = float2(p.x, 1.0 - p.y);
    return o;
}

// The new strand over what is left of the trail.
fragment float4 accumulate_fragment(Screen in [[stage_in]], float4 trail [[color(0)]],
                                    texture2d<float> layer [[texture(0)]],
                                    constant Post &post [[buffer(0)]]) {
    float4 l = layer.read(uint2(in.position.xy));
    return l + trail * post.decay * (1.0 - l.a);
}

// The trail of the smearing strands under the other strands' layer.
fragment float4 merge_fragment(Screen in [[stage_in]], float4 layer [[color(0)]],
                               texture2d<float> trail [[texture(0)]]) {
    return layer + trail.read(uint2(in.position.xy)) * (1.0 - layer.a);
}

// Half the size of the source; the first level keeps only what is bright.
fragment float4 bloom_down_fragment(Screen in [[stage_in]], texture2d<float> src [[texture(0)]],
                                    constant Post &post [[buffer(0)]]) {
    float2 o = post.texel;
    float3 c = 0.25 * (src.sample(linear_clamp, in.uv + float2(-o.x, -o.y)).rgb +
                       src.sample(linear_clamp, in.uv + float2(o.x, -o.y)).rgb +
                       src.sample(linear_clamp, in.uv + float2(-o.x, o.y)).rgb +
                       src.sample(linear_clamp, in.uv + float2(o.x, o.y)).rgb);
    if (post.threshold > 0.0) {
        float peak = max(c.r, max(c.g, c.b));
        c *= max(peak - post.threshold, 0.0) / max(peak, 1e-4);
    }
    return float4(c, 1.0);
}

// The smaller level, spread with a tent filter, added onto the larger.
fragment float4 bloom_up_fragment(Screen in [[stage_in]], texture2d<float> src [[texture(0)]],
                                  constant Post &post [[buffer(0)]]) {
    float2 o = post.texel;
    float3 c = 4.0 * src.sample(linear_clamp, in.uv).rgb;
    c += 2.0 * (src.sample(linear_clamp, in.uv + float2(o.x, 0)).rgb +
                src.sample(linear_clamp, in.uv - float2(o.x, 0)).rgb +
                src.sample(linear_clamp, in.uv + float2(0, o.y)).rgb +
                src.sample(linear_clamp, in.uv - float2(0, o.y)).rgb);
    c += src.sample(linear_clamp, in.uv + o).rgb + src.sample(linear_clamp, in.uv - o).rgb +
         src.sample(linear_clamp, in.uv + float2(o.x, -o.y)).rgb +
         src.sample(linear_clamp, in.uv + float2(-o.x, o.y)).rgb;
    return float4(c / 16.0, 1.0);
}

// Linear up to `knee`, then easing towards `top` without reaching it.
static float shoulder(float x, float knee, float top) {
    if (x <= knee) {
        return x;
    }
    float room = top - knee;
    return knee + room * (1.0 - exp(-(x - knee) / room));
}

static float3 srgb_encode(float3 c) {
    return select(1.055 * pow(c, 1.0 / 2.4) - 0.055, 12.92 * c, c <= 0.0031308);
}

fragment float4 final_fragment(Screen in [[stage_in]], texture2d<float> strand [[texture(0)]],
                               texture2d<float> bloom [[texture(1)]],
                               constant Post &post [[buffer(0)]]) {
    float2 ndc = float2(in.uv.x * 2.0 - 1.0, 1.0 - in.uv.y * 2.0);
    float3 d = normalize(float3(ndc.x * post.tan_half_fov * post.aspect, ndc.y * post.tan_half_fov, -1.0));
    float vignette = 1.0 - 0.35 * dot(ndc * 0.7, ndc * 0.7);
    float4 s = strand.read(uint2(in.position.xy));
    float3 c = backdrop(d, post.background.rgb) * vignette * (1.0 - s.a) + s.rgb;
    c += bloom.sample(linear_clamp, in.uv).rgb * post.bloom;

    float top = max(post.headroom, 1.0);
    float knee = top > 1.0 ? 1.0 : 0.75;
    float peak = max(c.r, max(c.g, c.b));
    if (peak > knee) {
        c *= shoulder(peak, knee, top) / peak;
    }
    if (post.srgb != 0) {
        c = srgb_encode(saturate(c));
    }
    return float4(c, 1.0);
}
