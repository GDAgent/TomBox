#pragma once
#include "sprite_atlas.h"
#include <vector>

// Borrowed view from coin_board. The game owns the tiles.
struct Board {
    const unsigned char* tiles;
    size_t width, height, player, coins;
};

struct Vertex { float x, y, z, w; float uv[4]; };

inline Sprite sprite_at(const Board& board, size_t cell) {
    if (cell == board.player) return Player;
    switch (board.tiles[cell]) {
        case '*': return Coin;
        case '#': return Wall;
        default: return Floor;
    }
}

namespace scene {
struct Point { float x, y, z; };
struct Camera { float x, z; };

inline Camera follow(const Board& b) {
    float px = float(b.player % b.width) + 0.5f, pz = float(b.player / b.width) + 0.5f;
    return {0.875f * float(b.width) / 2 + 0.125f * px,
            0.875f * float(b.height) / 2 + 0.125f * pz};
}

constexpr float floor_radius = 0.484f, player_radius = 0.29f, wall_radius = 0.45f, coin_radius = 0.31f;
constexpr float player_top = 0.52f, wall_top = 0.64f, coin_top = 0.24f;
// Per tile: 5 floor quads and at most 8 coin triangles plus 8 rim quads.
constexpr size_t max_vertices_per_tile = 5 * 6 + 8 * 9;

struct Geometry {
    std::vector<Vertex>& out;
    Camera camera;

    Vertex vertex(Point p, Sprite sprite, float u, float v, float light = 1) const {
        return {p.x - camera.x, p.y, p.z - camera.z, 1, {(float(sprite) + u) * 0.25f, v, light, 1}};
    }
    void quad(Vertex a, Vertex b, Vertex c, Vertex d) {
        out.insert(out.end(), {a, b, c, a, c, d});
    }

    void box(float x, float z, float r, float bottom, float top, Sprite s) {
        const Point corners[] = {{x-r,top,z-r}, {x+r,top,z-r}, {x+r,top,z+r}, {x-r,top,z+r}};
        quad(vertex(corners[0],s,0,0), vertex(corners[1],s,1,0),
             vertex(corners[2],s,1,1), vertex(corners[3],s,0,1));
        for (int i = 0; i < 4; ++i) {
            Point a = corners[i], b = corners[(i+1)%4];
            // Sample the middle brick/visor row on vertical faces. The floor edge is dark.
            float v = s == Floor ? 0.375f : 0.625f;
            constexpr float light[] = {0.75f, 0.4375f, 0.625f, 0.8125f};
            quad(vertex(a,s,0,v,light[i]), vertex(b,s,1,v,light[i]),
                 vertex({b.x,bottom,b.z},s,1,v,light[i]), vertex({a.x,bottom,a.z},s,0,v,light[i]));
        }
    }

    void coin(float x, float z) {
        constexpr float d = 0.70710678f;
        constexpr float ring[][2] = {{1,0},{d,d},{0,1},{-d,d},{-1,0},{-d,-d},{0,-1},{d,-d}};
        auto rim_vertex = [&](int i, float height, bool cap) {
            float u = ring[i][0], v = ring[i][1];
            return vertex({x+u*coin_radius, height, z+v*coin_radius}, Coin,
                          cap ? (u+1)/2 : 0.625f, cap ? (v+1)/2 : 0.375f, cap ? 1.0f : 0.625f-0.125f*u);
        };
        Vertex center = vertex({x,coin_top,z},Coin,0.5f,0.5f);
        for (int i = 0; i < 8; ++i) {
            int j = (i+1)%8;
            out.insert(out.end(), {center, rim_vertex(i,coin_top,true), rim_vertex(j,coin_top,true)});
            quad(rim_vertex(i,coin_top,false), rim_vertex(j,coin_top,false),
                 rim_vertex(j,0.06f,false), rim_vertex(i,0.06f,false));
        }
    }
};

inline void build(const Board& b, std::vector<Vertex>& out) {
    out.clear();
    Geometry mesh{out, follow(b)};
    for (size_t i = 0; i < b.width*b.height; ++i) {
        float x = float(i % b.width) + 0.5f, z = float(i / b.width) + 0.5f;
        switch (sprite_at(b, i)) {
            case Coin: mesh.coin(x, z); break;
            case Player: mesh.box(x, z, player_radius, 0, player_top, Player); break;
            case Wall: mesh.box(x, z, wall_radius, 0, wall_top, Wall); break;
            default: break;
        }
    }
    // Submit the floor last. Overlapping player, wall and coin pixels require depth writes.
    for (size_t i = 0; i < b.width*b.height; ++i)
        mesh.box(float(i % b.width)+0.5f, float(i / b.width)+0.5f, floor_radius, -0.13f, 0, Floor);
}

// Project known face-interior texels independently of the generated vertex stream.
inline int check_pixels(Frame& frame, const Board& b) {
    D3D12_RANGE read = {0, size_t(frame.pitch)*frame.height};
    void* mapped = nullptr;
    CHECK(frame.readback->Map(0, &read, &mapped));
    // The test board is 4x3. Its camera follows one eighth of the player's
    // displacement from the center, independent of the mesh builder.
    Camera camera{2 + (float(b.player % 4)-1.5f)/8, 1.5f + (float(b.player / 4)-1)/8};
    bool passed = true;
    auto probe = [&](Point p, size_t cell, Sprite s, UINT u, UINT v, float light = 1) {
        float z = p.z - camera.z, distance = 6 - 0.8f*p.y - 0.6f*z;
        int px = int(frame.width * (0.5f + 1.125f*(p.x-camera.x)/distance));
        int py = int(frame.height * (0.5f - 1.5f*(0.6f*p.y-0.8f*z)/distance));
        if (px < 0 || py < 0 || px >= int(frame.width) || py >= int(frame.height)) {
            passed = false;
            return;
        }
        Color expected = sprite_texel(s, u, v, b.coins == 1);
        auto* actual = static_cast<unsigned char*>(mapped) + py*frame.pitch + 4*px;
        bool match = actual[3] == expected[3];
        for (UINT channel = 0; channel < 3; ++channel) {
            // Allow one UNORM rounding step on shaded faces.
            expected[channel] = static_cast<unsigned char>(expected[channel]*light + 0.5f);
            int error = int(actual[channel]) - expected[channel];
            int tolerance = light == 1 ? 0 : 1;
            if (error < -tolerance || error > tolerance) match = false;
        }
        if (!match) {
            if (passed) std::fprintf(stderr, "3D frame %llu cell %zu pixel (%d,%d): got %u %u %u, expected %u %u %u\n",
                frame.frame, cell, px, py, actual[0], actual[1], actual[2], expected[0], expected[1], expected[2]);
            passed = false;
        }
    };
    for (size_t i = 0; i < b.width*b.height; ++i) {
        Sprite s = sprite_at(b, i);
        float x = float(i % b.width)+0.5f, z = float(i / b.width)+0.5f;
        float r = floor_radius, h = 0;
        if (s == Player) { r = player_radius; h = player_top; }
        if (s == Coin) { r = coin_radius; h = coin_top; }
        if (s == Wall) { r = wall_radius; h = wall_top; }
        probe({x-r*0.25f,h,z-r*0.25f}, i, s, 1, 1);
        // A tall piece on the next row can hide the front of an empty tile.
        if (s == Floor) probe({x+r*0.75f,0,z-r*0.75f}, i, s, 3, 0);
        else probe({x+r*0.25f,h,z+r*0.25f}, i, s, 2, 2);
        if (s == Player || s == Wall) {
            Point front{x+r*0.25f,h*0.5f,z+r};
            // On the back row the wall can hide the player's front face. Trace
            // this ray to the wall's top and check the covering brick texel.
            float t = (wall_top-front.y)/(4.8f-front.y);
            float wx = front.x + t*(camera.x-front.x);
            float wz = front.z + t*(camera.z+3.6f-front.z);
            float u = (wx-x+wall_radius)/(2*wall_radius);
            float v = (wz-z-1+wall_radius)/(2*wall_radius);
            bool wall_ahead = i+b.width < b.width*b.height && b.tiles[i+b.width] == '#';
            if (wall_ahead && u >= 0 && u < 1 && v >= 0 && v < 1)
                probe(front, i, Wall, UINT(4*u), UINT(4*v));
            else probe(front, i, s, 1, 2, 0.625f);
        }
    }
    D3D12_RANGE no_writes{};
    frame.readback->Unmap(0, &no_writes);
    return passed ? 0 : 1;
}
} // namespace scene
