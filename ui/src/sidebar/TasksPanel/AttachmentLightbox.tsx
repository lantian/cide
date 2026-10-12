/** Task metadata adapter for the shared fullscreen gallery. */
import { ImageViewer } from '@/overlays/ImageViewer'
import { formatBytes, type AttachmentPreview, type AttachmentView } from './model'

export interface AttachmentLightboxProps {
  images: readonly AttachmentView[]
  /** Removing the current attachment closes the viewer. */
  current: string
  previews: Readonly<Record<string, AttachmentPreview>>
  onClose: () => void
  onStep: (attachment: string) => void
  onOpen?: ((attachment: string) => void) | undefined
  onRequestPreview?: ((attachment: string) => void) | undefined
}

export function AttachmentLightbox({ images, ...props }: AttachmentLightboxProps) {
  return <ImageViewer {...props} auditPrefix="attachmentLightbox"
    images={images.map((image) => ({ id: image.id, title: image.name, previewId: image.id, meta: formatBytes(image.bytes) }))} />
}
