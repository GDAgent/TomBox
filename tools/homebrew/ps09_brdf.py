"""Scalar BRDF lookup integrals for three complete PS09 pixel shaders."""
from functools import lru_cache
import math
import struct


def hammersley_samples():
    for i in range(1024):
        # The ten bits of the reversed sample index are the fractional angle.
        azimuth = math.tau * int(f"{i:010b}"[::-1], 2) / 1024
        yield max(1e-6, i / 1024), math.sin(azimuth)


SAMPLES = tuple(hammersley_samples())


def beckmann_lambda(cosine, alpha):
    if cosine >= 1:
        return 0
    t = cosine / (alpha * math.sqrt(1 - cosine * cosine))
    if t >= 1.6:
        return 0
    return ((0.160632 * t - 0.502758) * t + 0.413333) / (((0.598172 * t + 0.812993) * t + 1.46523) * t)


def sheen_lambda(cosine, alpha):
    t = 1 - (1 - alpha) ** 2
    a = 25.3245 - 3.7771988*t
    b = 3.32435 + 0.50551987*t
    c = 0.16801 + 0.03022*t
    d = -1.27393 - 0.70367*t
    e = -4.85967 + 0.5391302*t

    def fit(x):
        return a / (1 + b * x ** c) + d * x + e

    log_lambda = fit(cosine) if cosine < 0.5 else 2 * fit(0.5) - fit(1 - cosine)
    return math.exp(log_lambda)


def integrate(index, nv, roughness):
    alpha = roughness * roughness
    slope = max(alpha, 0.001)
    view_x = math.sqrt(1 - nv*nv)
    fresnel = specular = diffuse = sheen = 0.0
    weighting = 0.0
    view_lambda = beckmann_lambda(nv, slope)
    view_sheen = sheen_lambda(nv, alpha)
    for u, sin_phi in SAMPLES:
        if index == 4906:
            nh = math.sqrt(1 / (1 - slope*slope * math.log1p(-u)))
        else:
            width = alpha if index == 4904 else slope
            nh = math.sqrt((1-u) / (1 + (width*width - 1)*u))
        vh = nv*nh + view_x * sin_phi * math.sqrt(max(0, 1 - nh*nh))
        nl = min(1, max(0, 2*vh*nh - nv))
        if nl > 0:
            vh = min(1, max(0, vh))
            if index == 4904:
                weight = nl
                specular += vh * weight
                weighting += weight
            elif index == 4906:
                weight = vh / (nh * nv * max(1e-4, 1 + view_lambda + beckmann_lambda(nl, slope)))
                specular += weight
            else:
                length_v = math.sqrt(slope*slope + (1 - slope*slope)*nv*nv)
                length_l = math.sqrt(slope*slope + (1 - slope*slope)*nl*nl)
                weight = 0.5 * nl * vh / (nh * (nl*length_v + nv*length_l))
                specular += weight
            fresnel += (1-vh)**5 * weight
        if index == 4904:
            continue

        # Cosine-weighted diffuse irradiance, with the same Hammersley points.
        nl = math.sqrt(1-u)
        vl = nv*nl + view_x * math.sqrt(u) * sin_phi
        fd90 = alpha * (1.5 + vl)
        diffuse += (1 + (fd90-1)*(1-nl)**5) * (1 + (fd90-1)*(1-nv)**5) * (1 - 0.33774835*alpha)

        if index == 4909:
            # The sheen lobe samples a uniform half vector on the hemisphere.
            nh = 1-u
            sin_sq = 1-nh*nh
            vh = nv*nh + view_x * math.sqrt(sin_sq) * sin_phi
            nl = min(1, max(0, 2*vh*nh - nv))
            if nl > 0:
                distribution = (2 + 1/alpha) * sin_sq ** (0.5/alpha) / math.tau
                visibility = 1 / max(1e-4, 4 * nv * nl * (1 + view_sheen + sheen_lambda(nl, alpha)))
                sheen += distribution * visibility * min(1, max(0, vh)) * nl
    if index == 4904:
        return fresnel/weighting, specular/weighting, 0, 0
    if index == 4906:
        # The low half of the diffuse export overwrites the scalar accumulator;
        # its upper 16 bits still supply the half-float alpha component.
        alpha_out, = struct.unpack("<e", struct.pack("<f", specular)[2:])
        return fresnel/1024, specular/1024, diffuse/1024, alpha_out
    return 4*fresnel/1024, 4*specular/1024, diffuse/1024, 4*math.tau*sheen/1024


def projected_cap(cosine, distance):
    # Irradiance from a spherical cap projected onto the visible hemisphere.
    inverse_square = min(0.9999, 1 / (distance*distance))
    cosine = max(-0.999, min(0.999, cosine))
    if cosine*cosine > inverse_square:
        return max(0, cosine) * inverse_square
    sine = math.sqrt(1 - cosine*cosine)
    slope = math.sqrt(1/inverse_square - 1)
    t = cosine * slope / sine
    chord = sine * math.sqrt(max(0, 1 - t*t))
    # The guest substitutes short minimax fits for acos and atan. Keep those
    # fits here: its final multiplication by distance squared magnifies their
    # absolute error, especially around the barely visible edge of a small cap.
    x = abs(t)
    acos_x = math.sqrt(1-x) * (((-0.0187293*x + 0.074261)*x - 0.21211439)*x + 1.57072878)
    angle = math.pi - acos_x if t > 0 else acos_x if t < 0 else math.pi/2
    ratio = chord/slope
    x = min(ratio, 1/ratio) if ratio else 0
    xx = x*x
    atan_small = x * (((((-0.01213232*xx + 0.05368138)*xx - 0.11735032)*xx
                         + 0.19389249)*xx - 0.33267564)*xx + 0.99997932)
    atan_ratio = atan_small if ratio <= 1 else math.pi/2 - atan_small
    integral = (cosine * angle - chord*slope) * inverse_square + atan_ratio
    return max(0, integral / math.pi)


def cap_pair(u, v):
    z = (2*u - 1) * abs(2*u - 1)
    distance = 1 + 99*v*v
    values = (projected_cap(z, distance)*distance**2,
              projected_cap(-z, distance**(1/3))*distance**(2/3))
    return (*values, 0, 0)


@lru_cache(maxsize=4)
def _grid(index):
    if index not in (4904, 4906, 4909, 4910):
        raise ValueError(f"unknown BRDF shader {index}")
    inputs, expected = bytearray(), []
    for y in range(16):
        for x in range(16):
            nv, roughness = (x+0.5)/16, (y+0.5)/16
            inputs.extend(struct.pack("<8f", nv, roughness, 0, 0, 0, 0, 0, 0))
            if index == 4910:
                for channel in cap_pair(nv, roughness):
                    unorm16 = round(65535 * min(1, max(0, channel)))
                    expected.append(unorm16 / 65535 * 255)
                continue
            for channel in integrate(index, nv, roughness):
                half, = struct.unpack("<e", struct.pack("<e", channel))
                expected.append(255 * max(0, min(1, half)))
    return bytes(inputs), tuple(expected)


def grid_reference(index):
    inputs, expected = _grid(index)
    return inputs, bytes(round(value) for value in expected), 0


def validate_grid(index, actual):
    # The float-to-UNORM rule allows 0.6 ULP on the integer side. Compare to
    # the unrounded fixed-point value to allow only the permitted codes.
    # https://microsoft.github.io/DirectX-Specs/d3d/archive/D3D11_3_FunctionalSpec.htm#FLOATtoUNORM
    _, expected = _grid(index)
    if len(actual) != len(expected):
        raise ValueError(f"shader {index}: got {len(actual)} bytes, expected {len(expected)}")
    largest = 0.0
    for i, (code, reference) in enumerate(zip(actual, expected)):
        error = abs(code - reference)
        if error > 0.6:
            raise ValueError(f"shader {index}, pixel {i // 4}, channel {'RGBA'[i % 4]}: "
                             f"got {code}, reference {reference:.5f}, exceeds 0.6 UNORM8 steps")
        largest = max(error, largest)
    return largest
