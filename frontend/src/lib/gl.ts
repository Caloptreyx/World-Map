// Small WebGL2 helpers for the 3D terrain: shader programs, textures and indexed meshes in the shared
// 12-byte vertex layout of mesh.ts.

import { type MeshData, VERTEX_BYTES } from './mesh.ts';

/** One uploaded mesh: its vertex array keeps the attribute layout and the index buffer binding. */
export interface GpuMesh {
  vao: WebGLVertexArrayObject;
  vertexBuffer: WebGLBuffer;
  indexBuffer: WebGLBuffer;
  vertexCount: number;
  indexCount: number;
}

function compileShader(gl: WebGL2RenderingContext, type: number, source: string): WebGLShader {
  const shader = gl.createShader(type);
  if (!shader) throw new Error('WebGL could not create a shader');
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS) && !gl.isContextLost()) {
    const log = gl.getShaderInfoLog(shader) ?? '';
    gl.deleteShader(shader);
    throw new Error(`WebGL shader failed to compile: ${log}`);
  }
  return shader;
}

/** Compiles and links a program; throws with the driver's log when that fails. */
export function createProgram(gl: WebGL2RenderingContext, vertexSource: string, fragmentSource: string): WebGLProgram {
  const vertex = compileShader(gl, gl.VERTEX_SHADER, vertexSource);
  let fragment: WebGLShader;
  try {
    fragment = compileShader(gl, gl.FRAGMENT_SHADER, fragmentSource);
  } catch (err) {
    gl.deleteShader(vertex);
    throw err;
  }
  const program = gl.createProgram();
  if (!program) {
    gl.deleteShader(vertex);
    gl.deleteShader(fragment);
    throw new Error('WebGL could not create a program');
  }
  gl.attachShader(program, vertex);
  gl.attachShader(program, fragment);
  gl.linkProgram(program);
  // The program keeps the compiled code; the shader objects are no longer needed.
  gl.deleteShader(vertex);
  gl.deleteShader(fragment);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS) && !gl.isContextLost()) {
    const log = gl.getProgramInfoLog(program) ?? '';
    gl.deleteProgram(program);
    throw new Error(`WebGL program failed to link: ${log}`);
  }
  return program;
}

export function uniformLocations<K extends string>(
  gl: WebGL2RenderingContext,
  program: WebGLProgram,
  names: readonly K[],
): Record<K, WebGLUniformLocation | null> {
  const locations = {} as Record<K, WebGLUniformLocation | null>;
  for (const name of names) locations[name] = gl.getUniformLocation(program, name);
  return locations;
}

/** An RGBA8 texture sampled with nearest filtering (one texel per chunk), or null when the context is lost. */
export function createTexture(
  gl: WebGL2RenderingContext,
  width: number,
  height: number,
  pixels: Uint8Array,
): WebGLTexture | null {
  const texture = gl.createTexture();
  if (!texture) return null;
  gl.bindTexture(gl.TEXTURE_2D, texture);
  gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, width, height, 0, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  return texture;
}

/**
 * Uploads a mesh in the shared layout. Attribute locations: 0 position (Int16 → float), 1 colour (normalised),
 * 2 face (integer), 3 shade (normalised). Returns null for an empty mesh or when the context is lost.
 */
export function uploadMesh(gl: WebGL2RenderingContext, mesh: MeshData): GpuMesh | null {
  if (mesh.indexCount === 0 || mesh.vertexCount === 0) return null;
  const vao = gl.createVertexArray();
  const vertexBuffer = gl.createBuffer();
  const indexBuffer = gl.createBuffer();
  if (!vao || !vertexBuffer || !indexBuffer) {
    if (vao) gl.deleteVertexArray(vao);
    if (vertexBuffer) gl.deleteBuffer(vertexBuffer);
    if (indexBuffer) gl.deleteBuffer(indexBuffer);
    return null;
  }
  gl.bindVertexArray(vao);
  gl.bindBuffer(gl.ARRAY_BUFFER, vertexBuffer);
  gl.bufferData(gl.ARRAY_BUFFER, mesh.vertices.subarray(0, mesh.vertexCount * VERTEX_BYTES), gl.STATIC_DRAW);
  gl.enableVertexAttribArray(0);
  gl.vertexAttribPointer(0, 3, gl.SHORT, false, VERTEX_BYTES, 0);
  gl.enableVertexAttribArray(1);
  gl.vertexAttribPointer(1, 3, gl.UNSIGNED_BYTE, true, VERTEX_BYTES, 6);
  gl.enableVertexAttribArray(2);
  gl.vertexAttribIPointer(2, 1, gl.UNSIGNED_BYTE, VERTEX_BYTES, 9);
  gl.enableVertexAttribArray(3);
  gl.vertexAttribPointer(3, 1, gl.UNSIGNED_BYTE, true, VERTEX_BYTES, 10);
  gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, indexBuffer);
  gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, mesh.indices.subarray(0, mesh.indexCount), gl.STATIC_DRAW);
  gl.bindVertexArray(null);
  gl.bindBuffer(gl.ARRAY_BUFFER, null);
  return { vao, vertexBuffer, indexBuffer, vertexCount: mesh.vertexCount, indexCount: mesh.indexCount };
}

export function deleteMesh(gl: WebGL2RenderingContext, mesh: GpuMesh): void {
  gl.deleteVertexArray(mesh.vao);
  gl.deleteBuffer(mesh.vertexBuffer);
  gl.deleteBuffer(mesh.indexBuffer);
}
