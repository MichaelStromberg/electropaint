// electropaint renderer shaders. Colours arrive final (lit and shaded on the
// CPU, as IrisGL did on the Indy); these shaders only project and rasterize.
#include <metal_stdlib>
using namespace metal;

struct Uniforms {
    float4x4 proj;
    float2 viewport;     // drawable size in pixels
    float px_scale;      // device pixels per original 1280x1024 screen pixel
    float line_width;    // in original pixels
    float4 fill_color;
    uint pattern[16];    // 16x16 stipple, row 0 = bottom, bit 15 = leftmost
    uint use_pattern;
};

struct Vertex {
    packed_float3 pos;
    packed_float4 color;
};

struct VOut {
    float4 position [[position]];
    float4 color;
};

vertex VOut tri_vertex(uint vid [[vertex_id]],
                       const device Vertex *verts [[buffer(0)]],
                       constant Uniforms &u [[buffer(1)]]) {
    Vertex v = verts[vid];
    VOut o;
    o.position = u.proj * float4(float3(v.pos), 1.0);
    o.color = float4(v.color);
    return o;
}

// Wide aliased lines the IrisGL way: each segment becomes a quad that is
// `line_width` pixels across its minor axis (vertical for x-major lines,
// horizontal for y-major lines). One instance per segment, six vertices.
vertex VOut line_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                        const device Vertex *verts [[buffer(0)]],
                        constant Uniforms &u [[buffer(1)]]) {
    Vertex a = verts[2 * iid], b = verts[2 * iid + 1];
    float4 ca = u.proj * float4(float3(a.pos), 1.0);
    float4 cb = u.proj * float4(float3(b.pos), 1.0);
    float2 sa = ca.xy / ca.w * 0.5 * u.viewport;
    float2 sb = cb.xy / cb.w * 0.5 * u.viewport;
    float2 d = sb - sa;
    float half_w = 0.5 * max(u.line_width * u.px_scale, 1.0);
    float2 off = abs(d.x) >= abs(d.y) ? float2(0.0, half_w) : float2(half_w, 0.0);
    // Corner table: (endpoint, side).
    const float2 corners[6] = { {0, -1}, {1, -1}, {1, 1}, {0, -1}, {1, 1}, {0, 1} };
    float2 c = corners[vid];
    float4 clip = c.x == 0 ? ca : cb;
    float2 shift = off * c.y / (0.5 * u.viewport) * clip.w;
    VOut o;
    o.position = float4(clip.xy + shift, clip.zw);
    o.color = float4(c.x == 0 ? a.color : b.color);
    return o;
}

// Full-viewport fill for clear(), the stippled fade and zclear().
vertex VOut fill_vertex(uint vid [[vertex_id]], constant Uniforms &u [[buffer(1)]]) {
    float2 p = float2((vid << 1) & 2, vid & 2);
    VOut o;
    o.position = float4(p * 2.0 - 1.0, 1.0, 1.0);
    o.color = u.fill_color;
    return o;
}

fragment float4 color_fragment(VOut in [[stage_in]]) {
    return in.color;
}

fragment float4 fill_fragment(VOut in [[stage_in]], constant Uniforms &u [[buffer(1)]]) {
    if (u.use_pattern != 0) {
        // Pattern pixels are original-screen pixels, counted from the bottom
        // left. A whole number of device pixels each keeps the cells uniform.
        float cell = max(floor(u.px_scale), 1.0);
        uint x = uint(in.position.x / cell);
        uint y = uint((u.viewport.y - in.position.y) / cell);
        uint row = u.pattern[y & 15];
        if (((row >> (15 - (x & 15))) & 1) == 0) {
            discard_fragment();
        }
    }
    return u.fill_color;
}

struct DepthOut {
    float depth [[depth(any)]];
};

fragment DepthOut zclear_fragment(VOut in [[stage_in]]) {
    DepthOut o;
    o.depth = 1.0;
    return o;
}
