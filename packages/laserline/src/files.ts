/**
 * Extensions of the model files `Mesh.parse` reads, lower case without the dot; the same
 * as `constants().modelExtensions`, but usable before `init`.
 */
export const MODEL_EXTENSIONS: readonly string[] = ['stl', '3mf', 'obj', 'ply', 'gltf', 'glb', 'step', 'stp'];

/** `accept` attribute of a file input for model files: `.stl,.3mf,…`. */
export const MODEL_ACCEPT = MODEL_EXTENSIONS.map((e) => `.${e}`).join(',');

/** Whether `fileName` has the extension of a model file (any case). */
export function isModelFile(fileName: string): boolean {
  const dot = fileName.lastIndexOf('.');
  return dot >= 0 && MODEL_EXTENSIONS.includes(fileName.slice(dot + 1).toLowerCase());
}

/** Contents of a file the user picked (e.g. from `<input type="file">` or a drop). */
export async function readFileBytes(file: Blob): Promise<Uint8Array> {
  return new Uint8Array(await file.arrayBuffer());
}

/** Offers `data` as a download named `fileName`. */
export function downloadBytes(data: Uint8Array | string, fileName: string, type = 'application/octet-stream'): void {
  const blob = new Blob([typeof data === 'string' ? data : new Uint8Array(data)], { type });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = fileName;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
