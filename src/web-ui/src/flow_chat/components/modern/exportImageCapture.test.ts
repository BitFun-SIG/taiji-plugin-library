// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  assertExportCanvasHasContent,
  captureExportCanvas,
  getExportCaptureStyleProperties,
} from './exportImageCapture';

const { domToCanvas } = vi.hoisted(() => ({ domToCanvas: vi.fn() }));
vi.mock('modern-screenshot', () => ({ domToCanvas }));
vi.mock('@/shared/utils/logger', () => ({ createLogger: () => ({ warn: vi.fn() }) }));

function raster(background: number, content = false, height = 65, transparent = false) {
  const canvas = document.createElement('canvas');
  canvas.width = 8;
  canvas.height = height;
  const getImageData = vi.fn((_x: number, y: number, width: number, rows: number) => {
    const data = new Uint8ClampedArray(width * rows * 4);
    for (let i = 0; i < data.length; i += 4) {
      data.set([background, background, background, transparent ? 0 : 255], i);
    }
    // Put content in the final strip to catch sparse sampling regressions.
    if (content && y + rows === height) data[data.length - 4] = background === 255 ? 0 : 255;
    return { data };
  });
  vi.spyOn(canvas, 'getContext').mockReturnValue({ getImageData } as unknown as CanvasRenderingContext2D);
  return { canvas, getImageData };
}

afterEach(() => {
  vi.restoreAllMocks();
  domToCanvas.mockReset();
  vi.useRealTimers();
});

describe('conversation image raster validation', () => {
  it.each([0, 28, 255])('rejects an opaque background-only capture (%s)', background => {
    expect(() => assertExportCanvasHasContent(raster(background).canvas)).toThrow('blank image');
  });

  it('rejects a transparent capture even when hidden RGB channels differ', () => {
    expect(() => assertExportCanvasHasContent(raster(0, true, 65, true).canvas)).toThrow('blank image');
  });

  it.each([0, 28, 255])('finds text contrast without reading a full long-image buffer (%s)', background => {
    const { canvas, getImageData } = raster(background, true);
    expect(() => assertExportCanvasHasContent(canvas)).not.toThrow();
    expect(getImageData.mock.calls.map(call => call[3])).toEqual([32, 32, 1]);
  });

  it('rejects zero-sized canvases', () => {
    const { canvas } = raster(0);
    canvas.width = 0;
    expect(() => assertExportCanvasHasContent(canvas)).toThrow('empty canvas');
  });
});

describe('conversation image capture', () => {
  it('keeps resolved paint/layout properties without serializing inherited theme variables', () => {
    const container = document.createElement('div');
    vi.spyOn(globalThis, 'getComputedStyle').mockReturnValue([
      'color', 'font-family', 'width', 'fill', '-webkit-text-fill-color', '--theme-color', '--theme-font',
    ] as unknown as CSSStyleDeclaration);
    expect(getExportCaptureStyleProperties(container)).toEqual([
      'color', 'font-family', 'width', 'fill', '-webkit-text-fill-color',
    ]);
  });

  it('validates before returning the canvas and passes the export filter to the rasterizer', async () => {
    const { canvas } = raster(255, true);
    domToCanvas.mockResolvedValue(canvas);
    const container = document.createElement('div');
    const filter = vi.fn(() => true);
    expect(await captureExportCanvas(container, 'white', filter)).toBe(canvas);
    expect(domToCanvas).toHaveBeenCalledExactlyOnceWith(container, expect.objectContaining({
      scale: 2, backgroundColor: 'white', filter, includeStyleProperties: expect.any(Array),
    }));
  });

  it.each(['blank', 'error'])('retries a %s capture at lower resolution', async failure => {
    const failed = raster(0).canvas;
    if (failure === 'blank') domToCanvas.mockResolvedValueOnce(failed);
    else domToCanvas.mockRejectedValueOnce(new Error('Raster allocation failed'));
    const recovered = raster(0, true).canvas;
    domToCanvas.mockResolvedValueOnce(recovered);
    expect(await captureExportCanvas(document.createElement('div'), 'black', () => true)).toBe(recovered);
    expect(domToCanvas.mock.calls.map(call => call[1].scale)).toEqual([2, 1]);
    if (failure === 'blank') expect([failed.width, failed.height]).toEqual([0, 0]);
  });

  it('rejects repeated blank output instead of returning a savable image', async () => {
    domToCanvas.mockImplementation(async () => raster(255).canvas);
    await expect(captureExportCanvas(document.createElement('div'), 'white', () => true))
      .rejects.toThrow('blank image');
    expect(domToCanvas).toHaveBeenCalledTimes(2);
  });

  it('does not start a concurrent retry after the capture times out', async () => {
    vi.useFakeTimers();
    domToCanvas.mockReturnValue(new Promise(() => {}));
    const capture = captureExportCanvas(document.createElement('div'), 'white', () => true);
    const rejection = expect(capture).rejects.toThrow('timed out');
    await vi.advanceTimersByTimeAsync(15_000);
    await rejection;
    expect(domToCanvas).toHaveBeenCalledTimes(1);
  });
});
