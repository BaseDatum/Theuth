import { useEffect, useRef, useState } from 'react'
import still from './pyramid-still.webp'

/*
 * The pyramid: the Great Pyramid as it stood c. 2560 BC, rendered with raw WebGL (four faces, one program), pinned
 * behind the page. True proportions: slope 51.84°, height / half-base 1.273. The faces are polished white Tura
 * limestone casing: near-planar, with hairline joints between courses so fine they fade out when the courses are a
 * few pixels apart; each face is indented a fraction of a degree along its centre line, so the two halves split the
 * light. A small gilded pyramidion caps the apex, with a faint screen-space halo. Lighting is physically based and
 * HDR: one low golden-hour sun (GGX specular, Schlick Fresnel), a hemisphere of faience-grey sky and warm ground
 * bounce, a Fresnel sheen of that sky in the polished stone, ground occlusion at the base. The floor carries the
 * sun-cast shadow (convex hull of the base and the projected apex) and a contact shadow, on a pool of lit sand.
 * ACES tone map, gamma 2.2, dithered. Scroll turns it ~110° about Y and eases the pitch; the motion is inertial.
 * Reduced motion renders a still frame. No WebGL (or no derivatives): the still photograph instead.
 */

const VERT = `
attribute vec3 aPos;
attribute vec3 aNor;
attribute vec2 aUV;
uniform mat4 uProj, uView, uModel;
uniform float uShift, uMode, uHeight;
uniform vec2 uGlow;
varying vec3 vN;
varying vec2 vUV;
varying vec3 vPos;
void main() {
  vUV = aUV;
  vec4 p;
  if (uMode > 1.5) {
    // apex halo: a screen-aligned quad of constant screen size at the apex
    p = uProj * uView * vec4(0.0, uHeight, 0.0, 1.0);
    p.xy += aPos.xy * uGlow * p.w;
  } else {
    vec4 w = uModel * vec4(aPos, 1.0);
    vPos = w.xyz;
    vN = mat3(uModel) * aNor;
    p = uProj * uView * w;
  }
  p.x += uShift * p.w;
  gl_Position = p;
}`

const FRAG = `
#extension GL_OES_standard_derivatives : enable
precision highp float;
varying vec3 vN;
varying vec2 vUV;
varying vec3 vPos;
uniform vec3 uCasing, uSand, uShadow;
uniform vec3 uSunDir, uEye;
uniform mat4 uModel;
uniform float uSlant, uHeight, uMode, uHalo;

const vec3 SUN = vec3(1.0, 0.74, 0.46) * 3.1;          // low sun, HDR
const vec3 SKY = vec3(0.40, 0.50, 0.54) * 0.50;        // faience-grey zenith
const vec3 HORIZON = vec3(0.70, 0.52, 0.34) * 0.60;    // golden-hour band at the horizon
const vec3 BOUNCE = vec3(0.36, 0.25, 0.14) * 0.50;     // warm sand bounce
const vec3 GOLD = vec3(1.0, 0.70, 0.24);               // electrum F0
const float PI = 3.14159265;

float hash(vec2 p) { return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
float vnoise(vec2 p) {
  vec2 i = floor(p), f = fract(p);
  f = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i + vec2(1, 0)), f.x), mix(hash(i + vec2(0, 1)), hash(i + vec2(1, 1)), f.x), f.y);
}
float fbm(vec2 p) {
  float v = 0.0, a = 0.5;
  for (int i = 0; i < 3; i++) { v += a * vnoise(p); p = p * 2.03 + 17.3; a *= 0.5; }
  return v;
}

// sky dome seen from any direction: warm horizon band under a cool zenith, sand below, a glow around the low sun
vec3 env(vec3 d) {
  vec3 up = mix(HORIZON, SKY, pow(clamp(d.y, 0.0, 1.0), 0.55));
  vec3 e = mix(BOUNCE, up, smoothstep(-0.15, 0.1, d.y));
  return e + SUN * 0.20 * pow(max(dot(d, uSunDir), 0.0), 5.0);
}

vec3 aces(vec3 x) { return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0); }
vec3 finish(vec3 hdr) {
  vec3 c = pow(aces(hdr), vec3(1.0 / 2.2));
  return c + (hash(gl_FragCoord.xy) - 0.5) / 255.0;
}

float sdTriangle(vec2 p, vec2 a, vec2 b, vec2 c) {
  vec2 e0 = b - a, e1 = c - b, e2 = a - c;
  vec2 v0 = p - a, v1 = p - b, v2 = p - c;
  vec2 pq0 = v0 - e0 * clamp(dot(v0, e0) / dot(e0, e0), 0.0, 1.0);
  vec2 pq1 = v1 - e1 * clamp(dot(v1, e1) / dot(e1, e1), 0.0, 1.0);
  vec2 pq2 = v2 - e2 * clamp(dot(v2, e2) / dot(e2, e2), 0.0, 1.0);
  float s = sign(e0.x * e2.y - e0.y * e2.x);
  vec2 d = min(min(vec2(dot(pq0, pq0), s * (v0.x * e0.y - v0.y * e0.x)),
                   vec2(dot(pq1, pq1), s * (v1.x * e1.y - v1.y * e1.x))),
                   vec2(dot(pq2, pq2), s * (v2.x * e2.y - v2.y * e2.x)));
  return -sqrt(d.x) * sign(d.y);
}

// GGX specular with correlated Smith visibility; returns D * V (F applied by the caller)
float ggx(float NdH, float NdL, float NdV, float rough) {
  float a2 = rough * rough * rough * rough;
  float d = NdH * NdH * (a2 - 1.0) + 1.0;
  float D = a2 / (PI * d * d);
  float V = 0.5 / (NdL * sqrt(NdV * NdV * (1.0 - a2) + a2) + NdV * sqrt(NdL * NdL * (1.0 - a2) + a2) + 1e-4);
  return D * V;
}

void main() {
  if (uMode > 1.5) {
    // apex halo: warm bloom, hot core, zero at the quad's edge
    float r2 = dot(vUV, vUV);
    float a = max(0.0, exp(-r2 * 4.0) * (1.0 - r2));
    vec3 c = mix(vec3(1.0, 0.76, 0.40), vec3(1.0, 0.95, 0.82), exp(-r2 * 14.0));
    gl_FragColor = vec4(c, a * uHalo + (hash(gl_FragCoord.xy) - 0.5) / 255.0);
    return;
  }

  if (uMode > 0.5) {
    // ── floor: cast shadow (hull of the base square and the projected apex), contact shadow, pool of lit sand ──
    mat3 R = mat3(uModel);
    vec3 local = vPos * R;                                    // world → model (rotation only)
    float sq = max(abs(local.x), abs(local.z)) - 1.0;         // distance outside the base square
    vec2 apex = -uSunDir.xz * (uHeight / uSunDir.y);
    vec2 c0 = (R * vec3(-1.0, 0.0, -1.0)).xz, c1 = (R * vec3(1.0, 0.0, -1.0)).xz;
    vec2 c2 = (R * vec3(1.0, 0.0, 1.0)).xz, c3 = (R * vec3(-1.0, 0.0, 1.0)).xz;
    float d = min(min(sdTriangle(vPos.xz, c0, c1, apex), sdTriangle(vPos.xz, c1, c2, apex)),
                  min(sdTriangle(vPos.xz, c2, c3, apex), sdTriangle(vPos.xz, c3, c0, apex)));
    d = min(d, sq);
    float far = length(vPos.xz);
    float pen = 0.04 + 0.10 * clamp((far - 1.0) / 3.0, 0.0, 1.0);   // penumbra widens away from the base
    float shadow = 1.0 - smoothstep(-pen, pen, d);
    float contact = 1.0 - smoothstep(0.0, 0.7, sq);
    float shade = max(shadow * 0.9, contact * 0.75);
    float pool = 1.0 - smoothstep(0.3, 3.6, far);
    vec3 lit = finish(uSand * (SUN * uSunDir.y + env(vec3(0.0, 1.0, 0.0))));
    float lightA = 0.8 * pool * (1.0 - shade);
    float shadeA = 0.65 * shade * (1.0 - smoothstep(1.8, 4.6, far));
    float alpha = lightA + shadeA;
    gl_FragColor = vec4((lightA * lit + shadeA * uShadow) / max(alpha, 1e-4), alpha);
    return;
  }

  // ── faces ─────────────────────────────────────────────────────────────────
  vec3 n0 = normalize(vN);
  vec3 T = normalize(cross(vec3(0.0, 1.0, 0.0), n0));        // along the base, face +x
  vec3 B = cross(n0, T);                                      // up the face
  float px = max(fwidth(vUV.y), 1e-5);                        // face units per pixel

  // casing courses: ~96 to the apex, blocks about twice as wide as tall, staggered per course
  float xw = vUV.x * (1.0 - vUV.y / uSlant);                  // horizontal position in half-base units
  float courseH = uSlant / 96.0;
  float blockW = courseH * 2.1;
  float row = floor(vUV.y / courseH);
  float xu = xw / blockW + hash(vec2(row, 3.0)) * 1.7;
  vec2 id = vec2(floor(xu), row);
  float fy = fract(vUV.y / courseH), fx = fract(xu);
  float m = min(min(fy, 1.0 - fy) * courseH, min(fx, 1.0 - fx) * blockW);
  float joint = (1.0 - smoothstep(0.0, 0.8 * px, m)) * smoothstep(2.5, 6.0, courseH / px);

  // faint concavity along the centre line; polished blocks each set a hair differently
  float side = smoothstep(-px, px, xw) * 2.0 - 1.0;
  vec3 n = normalize(n0 - 0.013 * side * T + 0.003 * ((hash(id) - 0.5) * T + (hash(id + 7.0) - 0.5) * B));

  // pyramidion: the top ~6% of the slant, with a hairline seam where it meets the casing
  float capY = uSlant * 0.94;
  float gold = smoothstep(capY - px, capY + px, vUV.y);
  float seam = 1.0 - smoothstep(0.0, 1.5 * px, abs(vUV.y - capY));

  vec3 V = normalize(uEye - vPos);
  vec3 L = uSunDir;
  vec3 H = normalize(L + V);
  float NdL = max(dot(n, L), 0.0), NdV = max(dot(n, V), 1e-3);
  float NdH = max(dot(n, H), 0.0), VdH = max(dot(V, H), 0.0);
  vec3 refl = env(reflect(-V, n));
  float ao = mix(0.45, 1.0, smoothstep(0.0, 0.5, vUV.y));    // the ground hides half the sky at the base

  // polished limestone: dielectric, near-white, a breath of tonal variation per block and across the face
  vec3 albedo = uCasing * (0.965 + 0.035 * fbm(vec2(xw * 4.0, vUV.y * 4.0))) * (1.0 + 0.03 * (hash(id + 3.0) - 0.5));
  vec3 F = vec3(0.05) + 0.95 * pow(1.0 - VdH, 5.0);
  vec3 Fv = vec3(0.05) + 0.95 * pow(1.0 - NdV, 5.0);
  // a tight lobe for the polish and a broad one for the soft sheen that sweeps the face as it turns
  float spec = ggx(NdH, NdL, NdV, 0.28) + 0.6 * ggx(NdH, NdL, NdV, 0.6);
  vec3 stone = SUN * NdL * (albedo * (1.0 - F) / PI + F * spec)
             + albedo * env(n) * ao
             + refl * Fv * 1.6 * ao;
  stone *= 1.0 - 0.22 * joint;

  // gilded cap: metal, sun glint plus the sky in it
  vec3 Fg = GOLD + (1.0 - GOLD) * pow(1.0 - VdH, 5.0);
  vec3 Fgv = GOLD + (1.0 - GOLD) * pow(1.0 - NdV, 5.0);
  vec3 cap = SUN * NdL * Fg * ggx(NdH, NdL, NdV, 0.22) + refl * Fgv * 1.6 + GOLD * (SUN * NdL * 0.5 + env(n) * 1.1);

  vec3 col = mix(stone, cap, gold) * (1.0 - 0.3 * seam);
  col = mix(col, HORIZON * 2.2, 0.10 * (1.0 - smoothstep(0.0, 0.55, vUV.y)));   // dust haze near the ground
  gl_FragColor = vec4(finish(col), 1.0);
}`

const HALF = 1
const HEIGHT = 146.6 / 115.15   // Great Pyramid: 146.6 m over a 230.3 m base → 51.84°
const SLANT = Math.hypot(HEIGHT, HALF)

type M4 = Float32Array

function perspective(fovY: number, aspect: number, near: number, far: number): M4 {
  const f = 1 / Math.tan(fovY / 2)
  const nf = 1 / (near - far)
  return new Float32Array([f / aspect, 0, 0, 0, 0, f, 0, 0, 0, 0, (far + near) * nf, -1, 0, 0, 2 * far * near * nf, 0])
}

function lookAt(eye: number[], at: number[]): M4 {
  let zx = eye[0]! - at[0]!, zy = eye[1]! - at[1]!, zz = eye[2]! - at[2]!
  let l = Math.hypot(zx, zy, zz)
  zx /= l; zy /= l; zz /= l
  // x = up × z, with up = (0,1,0)
  let xx = zz, xz = -zx
  l = Math.hypot(xx, xz)
  xx /= l; xz /= l
  const yx = zy * xz, yy = zz * xx - zx * xz, yz = -zy * xx
  return new Float32Array([
    xx, yx, zx, 0,
    0, yy, zy, 0,
    xz, yz, zz, 0,
    -(xx * eye[0]! + xz * eye[2]!), -(yx * eye[0]! + yy * eye[1]! + yz * eye[2]!), -(zx * eye[0]! + zy * eye[1]! + zz * eye[2]!), 1,
  ])
}

function rotY(a: number): M4 {
  const c = Math.cos(a), s = Math.sin(a)
  return new Float32Array([c, 0, -s, 0, 0, 1, 0, 0, s, 0, c, 0, 0, 0, 0, 1])
}

/** Four lateral faces, a floor quad for the shadow, a unit quad for the apex halo: [pos3, nor3, uv2] per vertex. */
function geometry(): Float32Array {
  const v: number[] = []
  for (let i = 0; i < 4; i++) {
    const a = (i * Math.PI) / 2
    const c = Math.cos(a), s = Math.sin(a)
    // face i is the +z face rotated by a: base corners at (±1, 0, 1), apex at (0, h, 0)
    const rot = (x: number, z: number) => [x * c + z * s, -x * s + z * c] as const
    const [lx, lz] = rot(-HALF, HALF)
    const [rx, rz] = rot(HALF, HALF)
    const ny = HALF / SLANT, nz = HEIGHT / SLANT
    const [nx, nzz] = rot(0, nz)
    const n = [nx, ny, nzz]
    v.push(0, HEIGHT, 0, ...n, 0, SLANT)
    v.push(lx, 0, lz, ...n, -1, 0)
    v.push(rx, 0, rz, ...n, 1, 0)
  }
  const q = [[-1, -1], [1, -1], [1, 1], [-1, -1], [1, 1], [-1, 1]] as const
  for (const [x, z] of q) v.push(x * 5, 0, z * 5, 0, 1, 0, 0, 0)
  for (const [x, y] of q) v.push(x, y, 0, 0, 0, 1, x, y)
  return new Float32Array(v)
}

function css(name: string): [number, number, number] {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim().split(/\s+/).map(Number)
  return [(v[0] ?? 200) / 255, (v[1] ?? 180) / 255, (v[2] ?? 140) / 255]
}

function start(canvas: HTMLCanvasElement): (() => void) | null {
  const gl = canvas.getContext('webgl', { alpha: true, antialias: true, premultipliedAlpha: false, powerPreference: 'low-power' })
  if (!gl || !gl.getExtension('OES_standard_derivatives')) return null

  const compile = (type: number, src: string) => {
    const sh = gl.createShader(type)!
    gl.shaderSource(sh, src)
    gl.compileShader(sh)
    if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(sh) ?? 'shader')
    return sh
  }
  const prog = gl.createProgram()!
  gl.attachShader(prog, compile(gl.VERTEX_SHADER, VERT))
  gl.attachShader(prog, compile(gl.FRAGMENT_SHADER, FRAG))
  gl.linkProgram(prog)
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog) ?? 'link')
  gl.useProgram(prog)

  const buf = gl.createBuffer()
  gl.bindBuffer(gl.ARRAY_BUFFER, buf)
  gl.bufferData(gl.ARRAY_BUFFER, geometry(), gl.STATIC_DRAW)
  const stride = 8 * 4
  const attr = (name: string, size: number, offset: number) => {
    const loc = gl.getAttribLocation(prog, name)
    gl.enableVertexAttribArray(loc)
    gl.vertexAttribPointer(loc, size, gl.FLOAT, false, stride, offset)
  }
  attr('aPos', 3, 0)
  attr('aNor', 3, 12)
  attr('aUV', 2, 24)

  const u = (name: string) => gl.getUniformLocation(prog, name)
  const uProj = u('uProj'), uView = u('uView'), uModel = u('uModel'), uShift = u('uShift'), uMode = u('uMode')
  const uEye = u('uEye'), uGlow = u('uGlow')
  gl.uniform1f(u('uSlant'), SLANT)
  gl.uniform1f(u('uHeight'), HEIGHT)
  // a low sun, 24° up, 47° to the left of the camera; fixed to the view, so the faces turn through it
  const az = -0.83, el = 0.42
  gl.uniform3f(u('uSunDir'), Math.sin(az) * Math.cos(el), Math.sin(el), Math.cos(az) * Math.cos(el))

  const theme = () => {
    gl.uniform3fv(u('uCasing'), css('--casing'))
    gl.uniform3fv(u('uSand'), css('--sand'))
    gl.uniform3fv(u('uShadow'), css('--shadow-rgb'))
    gl.uniform1f(u('uHalo'), parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--halo')) || 0.5)
  }
  theme()

  gl.enable(gl.BLEND)
  gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA)
  gl.clearColor(0, 0, 0, 0)

  const still = matchMedia('(prefers-reduced-motion: reduce)').matches
  let w = 0, h = 0
  const resize = () => {
    const dpr = Math.min(2, devicePixelRatio || 1)
    w = canvas.clientWidth
    h = canvas.clientHeight
    canvas.width = Math.round(w * dpr)
    canvas.height = Math.round(h * dpr)
    gl.viewport(0, 0, canvas.width, canvas.height)
    gl.uniformMatrix4fv(uProj, false, perspective(0.46, w / h, 0.5, 30))
    gl.uniform2f(uGlow, (0.13 * h) / w, 0.13)
  }

  const progress = () => scrollY / Math.max(1, document.documentElement.scrollHeight - innerHeight)
  let cur = progress(), target = cur, drift = 0, last = performance.now(), raf = 0

  const draw = (p: number) => {
    const wide = w >= 768
    // right-weighted on desktop, centered behind the headline on mobile
    gl.uniform1f(uShift, wide ? 0.44 + 0.2 * p : 0.0)
    const scale = wide ? Math.min(1.1, Math.max(0.7, Math.min(w, h) / 900)) : Math.min(1, Math.max(0.55, w / 480))
    const pitch = (14 - 8 * p) * (Math.PI / 180)
    const dist = (7.2 + 1.4 * p) / scale
    // aim a little above the centroid so the pyramid sits in the lower half of the frame, on its shadow
    const eye = [0, 0.7 + Math.sin(pitch) * dist, Math.cos(pitch) * dist]
    gl.uniform3fv(uEye, eye)
    gl.uniformMatrix4fv(uView, false, lookAt(eye, [0, 0.7, 0]))
    gl.uniformMatrix4fv(uModel, false, rotY((-32 + 110 * p) * (Math.PI / 180) + drift))
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT)
    gl.uniform1f(uMode, 1)
    gl.drawArrays(gl.TRIANGLES, 12, 6)          // floor: shadow and lit sand, before the faces
    gl.enable(gl.DEPTH_TEST)
    gl.uniform1f(uMode, 0)
    gl.drawArrays(gl.TRIANGLES, 0, 12)          // faces
    gl.disable(gl.DEPTH_TEST)
    gl.uniform1f(uMode, 2)
    gl.drawArrays(gl.TRIANGLES, 18, 6)          // apex halo, over everything
  }

  const frame = (now: number) => {
    const dt = Math.min(0.05, (now - last) / 1000)
    last = now
    target = progress()
    cur += (target - cur) * Math.min(1, dt * 4.5)
    drift += dt * 0.012
    draw(cur)
    raf = requestAnimationFrame(frame)
  }

  const onResize = () => {
    resize()
    if (still) draw(progress())
  }
  const onScheme = () => {
    theme()
    if (still) draw(progress())
  }
  const scheme = matchMedia('(prefers-color-scheme: light)')
  resize()
  addEventListener('resize', onResize)
  scheme.addEventListener('change', onScheme)
  if (still) {
    draw(cur)
    // still frame follows the document position, but only when it actually changes
    addEventListener('scroll', onResize, { passive: true })
  } else {
    raf = requestAnimationFrame(frame)
  }
  return () => {
    cancelAnimationFrame(raf)
    removeEventListener('resize', onResize)
    removeEventListener('scroll', onResize)
    scheme.removeEventListener('change', onScheme)
  }
}

export function Pyramid() {
  const ref = useRef<HTMLCanvasElement>(null)
  const [fallback, setFallback] = useState(false)

  useEffect(() => {
    const canvas = ref.current
    if (!canvas) return
    try {
      const stop = start(canvas)
      if (!stop) setFallback(true)
      return stop ?? undefined
    } catch (e) {
      console.error('pyramid: falling back to the still', e)
      setFallback(true)
      return undefined
    }
  }, [])

  return (
    <div className="stage" aria-hidden>
      {fallback ? (
        <img
          src={still}
          alt=""
          className="absolute top-1/2 right-[4vw] w-[min(46vw,720px)] -translate-y-1/2 opacity-90 mix-blend-lighten max-md:right-1/2 max-md:w-[80vw] max-md:translate-x-1/2 max-md:opacity-30"
        />
      ) : (
        <canvas ref={ref} className="block h-full w-full max-md:opacity-30" />
      )}
    </div>
  )
}
