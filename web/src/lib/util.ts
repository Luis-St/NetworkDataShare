/** Random id that also works outside secure contexts (crypto.randomUUID needs HTTPS). */
export function uid(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

/** Short "Browser on OS" description of this device. */
export function deviceHint(): string {
  const ua = navigator.userAgent;
  let os = 'Unknown OS';
  if (/Android/i.test(ua)) os = 'Android';
  else if (/iPhone|iPod/i.test(ua)) os = 'iPhone';
  else if (/iPad/i.test(ua) || (/Macintosh/i.test(ua) && navigator.maxTouchPoints > 1)) os = 'iPad';
  else if (/CrOS/i.test(ua)) os = 'ChromeOS';
  else if (/Windows/i.test(ua)) os = 'Windows';
  else if (/Macintosh|Mac OS X/i.test(ua)) os = 'macOS';
  else if (/Linux/i.test(ua)) os = 'Linux';

  let browser = 'Browser';
  if (/Edg\//i.test(ua)) browser = 'Edge';
  else if (/OPR\/|Opera/i.test(ua)) browser = 'Opera';
  else if (/SamsungBrowser/i.test(ua)) browser = 'Samsung Internet';
  else if (/Firefox|FxiOS/i.test(ua)) browser = 'Firefox';
  else if (/Chrome|CriOS|Chromium/i.test(ua)) browser = 'Chrome';
  else if (/Safari/i.test(ua)) browser = 'Safari';

  return `${browser} on ${os}`;
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = bytes / 1024;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i++;
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[i]}`;
}

export function formatTime(ts: number): string {
  return new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

export type Segment = { text: string; href?: string };

const URL_RE = /\b(?:https?:\/\/|www\.)[^\s<>"']+/gi;

/** Splits text into plain and link segments. Trailing punctuation is kept out of links. */
export function linkify(text: string): Segment[] {
  const out: Segment[] = [];
  let last = 0;
  for (const match of text.matchAll(URL_RE)) {
    let url = match[0];
    const start = match.index ?? 0;
    const count = (c: string) => url.split(c).length - 1;
    while (/[.,;:!?)\]}]$/.test(url)) {
      // Keep a closing parenthesis that belongs to the URL, as in wiki/Foo_(bar).
      if (url.endsWith(')') && count('(') >= count(')')) break;
      url = url.slice(0, -1);
    }
    if (start > last) out.push({ text: text.slice(last, start) });
    out.push({ text: url, href: url.startsWith('www.') ? `https://${url}` : url });
    last = start + url.length;
  }
  if (last < text.length) out.push({ text: text.slice(last) });
  return out;
}

export const canWriteClipboard = typeof navigator.clipboard?.writeText === 'function';
export const canCopyImages = window.isSecureContext && typeof ClipboardItem !== 'undefined';

export async function copyText(text: string): Promise<boolean> {
  if (canWriteClipboard) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      /* fall through to the legacy path */
    }
  }
  const area = document.createElement('textarea');
  area.value = text;
  area.style.position = 'fixed';
  area.style.opacity = '0';
  document.body.appendChild(area);
  area.select();
  let ok = false;
  try {
    ok = document.execCommand('copy');
  } catch {
    ok = false;
  }
  area.remove();
  return ok;
}

/** Copies an image to the clipboard, converting it to PNG (the only image type browsers accept). */
export async function copyImage(url: string): Promise<boolean> {
  if (!canCopyImages) return false;
  try {
    const png = (async () => {
      const blob = await (await fetch(url)).blob();
      if (blob.type === 'image/png') return blob;
      const bitmap = await createImageBitmap(blob);
      const canvas = document.createElement('canvas');
      canvas.width = bitmap.width;
      canvas.height = bitmap.height;
      canvas.getContext('2d')!.drawImage(bitmap, 0, 0);
      return await new Promise<Blob>((resolve, reject) =>
        canvas.toBlob((b) => (b ? resolve(b) : reject(new Error('encode failed'))), 'image/png'),
      );
    })();
    // Passing a promise keeps Safari's user-gesture requirement satisfied.
    await navigator.clipboard.write([new ClipboardItem({ 'image/png': png })]);
    return true;
  } catch {
    return false;
  }
}

/** Starts a browser download of a URL (the browser streams it to disk). */
export function triggerDownload(url: string, name: string) {
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.rel = 'noopener';
  document.body.appendChild(a);
  a.click();
  a.remove();
}
