import { useEffect, useRef, useState } from 'react'
import still from './pyramid-still.webp'

/*
 * The pyramid: a square-base limestone pyramid rendered with raw WebGL (four faces, one program), pinned behind the
 * page. Stone is procedural in the fragment shader: courses of blocks with staggered joints, soot grout, per-block
 * tone, limestone grain, weathering streaks, a bevel catch on the four arrises. One warm key from the upper left, a
 * faint cold fill from below, a soft contact shadow on an invisible floor. Scroll turns it ~110° about Y and eases the
 * pitch; the motion is inertial. Reduced motion renders a still frame. No WebGL: the still photograph instead.
 */

const VERT = `
attribute vec3 aPos;
attribute vec3 aNor;
attribute vec2 aUV;
uniform mat4 uProj, uView, uModel;
uniform float uShift;
varying vec3 vN;
varying vec2 vUV;
varying vec3 vPos;
void main() {
  vec4 w = uModel * vec4(aPos, 1.0);
  vPos = w.xyz;
  vN = mat3(uModel) * aNor;
  vUV = aUV;
  vec4 p = uProj * uView * w;
  p.x += uShift * p.w;
  gl_Position = p;
}`

const FRAG = `
precision highp float;
varying vec3 vN;
varying vec2 vUV;
varying vec3 vPos;
uniform vec3 uStone, uStoneDim, uGrout, uKey, uFill;
uniform vec3 uKeyDir, uFillDir;
uniform float uSlant;
uniform float uGround;

float hash(vec2 p) { return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
float vnoise(vec2 p) {
  vec2 i = floor(p), f = fract(p);
  f = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i + vec2(1, 0)), f.x), mix(hash(i + vec2(0, 1)), hash(i + vec2(1, 1)), f.x), f.y);
}
float fbm(vec2 p) {
  float v = 0.0, a = 0.5;
  for (int i = 0; i < 4; i++) { v += a * vnoise(p); p = p * 2.03 + 17.3; a *= 0.5; }
  return v;
}

void main() {
  if (uGround > 0.5) {
    // contact shadow: soft dark ellipse on the floor, pushed away from the key light
    vec2 c = vPos.xz - vec2(0.22, -0.18);
    float d = length(c * vec2(0.85, 1.0));
    float a = 0.6 * (1.0 - smoothstep(0.5, 1.5, d));
    gl_FragColor = vec4(uGrout, a);
    return;
  }

  // ── masonry ───────────────────────────────────────────────────────────────
  vec2 uv = vUV + 0.003 * (vec2(vnoise(vUV * 9.0), vnoise(vUV * 9.0 + 5.0)) - 0.5);   // hand-cut joints wander
  float courseH = 0.036;
  float row = floor(uv.y / courseH);
  float fy = fract(uv.y / courseH);
  float blockW = 0.075 + 0.05 * hash(vec2(row, 7.0));
  float xu = uv.x / blockW + hash(vec2(row, 3.0)) * 1.7;
  float col = floor(xu);
  float fx = fract(xu);
  vec2 id = vec2(col, row);
  float bh = hash(id);

  float mx = min(fx, 1.0 - fx) * blockW;
  float my = min(fy, 1.0 - fy) * courseH;
  float m = min(mx, my);
  float groutW = 0.0016 + 0.0012 * hash(id + 9.0);
  float grout = 1.0 - smoothstep(0.0, groutW, m);
  float ao = 1.0 - 0.30 * (1.0 - smoothstep(groutW, groutW + 0.009, m));

  // ── stone ─────────────────────────────────────────────────────────────────
  float grain = fbm(vUV * 70.0 + bh * 10.0);
  float pits = smoothstep(0.6, 0.9, vnoise(vUV * 420.0));
  float tone = mix(0.55, 1.0, bh) * (0.86 + 0.28 * grain) - 0.14 * pits;
  vec3 stone = mix(uStoneDim, uStone, clamp(tone, 0.0, 1.0));
  float streak = fbm(vec2(vUV.x * 2.2, vUV.y * 0.7) + 31.0);
  stone = mix(stone, uStoneDim, 0.5 * smoothstep(0.5, 0.85, streak));
  stone *= mix(0.78, 1.0, smoothstep(0.0, 0.45, vUV.y));   // dust and floor occlusion at the base

  // ── arrises: distance to the face's slanted edges, in face units ──────────
  float edge = (1.0 - vUV.y / uSlant) - abs(vUV.x);
  float bevel = 1.0 - smoothstep(0.0, 0.03, edge);
  float chip = smoothstep(0.3, 0.7, vnoise(vUV * 90.0));

  // ── lighting ──────────────────────────────────────────────────────────────
  vec3 jitter = vec3(hash(id + 1.0), hash(id + 2.0), hash(id + 3.0)) - 0.5;
  vec3 n = normalize(vN + 0.07 * jitter + 0.10 * bevel * vec3(sign(vUV.x), 0.3, 0.0));
  float key = max(0.0, dot(n, uKeyDir));
  float fill = max(0.0, dot(n, uFillDir));
  float wrap = 0.5 + 0.5 * dot(n, uKeyDir);           // soft self-shadow wraps past the terminator
  float relief = 1.0 + 0.10 * (fy - 0.5) * (1.0 - grout);          // each block reads as a lip: lighter top, darker bottom
  vec3 lit = stone * (0.20 + 0.14 * wrap + 0.66 * key * uKey) * ao * relief;
  lit += stone * uFill * 0.34 * fill;
  lit = mix(lit, lit * (1.0 - 0.35 * chip), bevel);
  lit = mix(lit, uGrout, grout * 0.9);
  gl_FragColor = vec4(lit, 1.0);
}`

const HALF = 1
const HEIGHT = 1.27
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

/** Four lateral faces plus a floor quad for the shadow: [pos3, nor3, uv2] per vertex. */
function geometry(): { faces: Float32Array; floor: Float32Array } {
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
  const f: number[] = []
  const q = [[-3, -3], [3, -3], [3, 3], [-3, -3], [3, 3], [-3, 3]]
  for (const [x, z] of q) f.push(x!, 0, z!, 0, 1, 0, 0, 0)
  return { faces: new Float32Array(v), floor: new Float32Array(f) }
}

function css(name: string): [number, number, number] {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim().split(/\s+/).map(Number)
  return [(v[0] ?? 200) / 255, (v[1] ?? 180) / 255, (v[2] ?? 140) / 255]
}

function unit(x: number, y: number, z: number): [number, number, number] {
  const l = Math.hypot(x, y, z)
  return [x / l, y / l, z / l]
}

function start(canvas: HTMLCanvasElement): (() => void) | null {
  const gl = canvas.getContext('webgl', { alpha: true, antialias: true, premultipliedAlpha: false, powerPreference: 'low-power' })
  if (!gl) return null

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

  const { faces, floor } = geometry()
  const buf = gl.createBuffer()
  gl.bindBuffer(gl.ARRAY_BUFFER, buf)
  const all = new Float32Array(faces.length + floor.length)
  all.set(faces)
  all.set(floor, faces.length)
  gl.bufferData(gl.ARRAY_BUFFER, all, gl.STATIC_DRAW)
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
  const uProj = u('uProj'), uView = u('uView'), uModel = u('uModel'), uShift = u('uShift'), uGround = u('uGround')
  gl.uniform1f(u('uSlant'), SLANT)
  gl.uniform3fv(u('uKeyDir'), unit(-0.62, 0.62, 0.48))
  gl.uniform3fv(u('uFillDir'), unit(0.25, -1, 0.35))
  gl.uniform3f(u('uKey'), 1.0, 0.92, 0.8)
  gl.uniform3f(u('uFill'), 0.36, 0.56, 0.54)

  const theme = () => {
    gl.uniform3fv(u('uStone'), css('--stone'))
    gl.uniform3fv(u('uStoneDim'), css('--stone-dim'))
    gl.uniform3fv(u('uGrout'), css('--grout-rgb'))
  }
  theme()

  gl.enable(gl.DEPTH_TEST)
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
    gl.uniformMatrix4fv(uView, false, lookAt([0, 0.7 + Math.sin(pitch) * dist, Math.cos(pitch) * dist], [0, 0.7, 0]))
    gl.uniformMatrix4fv(uModel, false, rotY((-32 + 110 * p) * (Math.PI / 180) + drift))
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT)
    gl.uniform1f(uGround, 1)
    gl.depthMask(false)
    gl.drawArrays(gl.TRIANGLES, 12, 6)
    gl.depthMask(true)
    gl.uniform1f(uGround, 0)
    gl.drawArrays(gl.TRIANGLES, 0, 12)
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
    gl.getExtension('WEBGL_lose_context')?.loseContext()
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
    } catch {
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
