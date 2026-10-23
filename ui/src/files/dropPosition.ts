/** WKWebView sends Cocoa points; GTK and Windows send physical pixels. */
export function dropCoordinateScale(userAgent: string, devicePixelRatio: number): number {
  return userAgent.includes('Macintosh') ? 1 : devicePixelRatio
}
