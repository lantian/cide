/** Image coordinates are measured in natural pixels; offsets are viewport pixels. */
export interface ImageSize { width: number; height: number }
export interface ImageCamera { scale: number; x: number; y: number }

export function fitImage(image: ImageSize, viewport: ImageSize): number {
  if (image.width <= 0 || image.height <= 0 || viewport.width <= 0 || viewport.height <= 0) return 1
  return Math.min(1, viewport.width / image.width, viewport.height / image.height)
}

export function constrainImage(camera: ImageCamera, image: ImageSize, viewport: ImageSize): ImageCamera {
  const scale = Math.max(Math.min(0.1, fitImage(image, viewport)), Math.min(16, camera.scale))
  const maxX = Math.max(0, (image.width * scale - viewport.width) / 2)
  const maxY = Math.max(0, (image.height * scale - viewport.height) / 2)
  return { scale, x: Math.max(-maxX, Math.min(maxX, camera.x)), y: Math.max(-maxY, Math.min(maxY, camera.y)) }
}

/** Keep the image point under the pointer fixed, unless an edge needs constraining. */
export function zoomImage(camera: ImageCamera, scale: number, point: { x: number; y: number }, image: ImageSize, viewport: ImageSize): ImageCamera {
  const bounded = constrainImage({ ...camera, scale }, image, viewport).scale
  const ratio = bounded / camera.scale
  return constrainImage({ scale: bounded, x: point.x - (point.x - camera.x) * ratio, y: point.y - (point.y - camera.y) * ratio }, image, viewport)
}
