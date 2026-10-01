/* Emits src-tauri/icons/icon.ico (32x32, 32bpp BMP-in-ICO) so tauri-build can
   embed a Windows resource icon. Brand color #335FE8 with a white monogram bar. */
#include <stdio.h>
#include <stdint.h>

static void put16(FILE *f, uint16_t v) { fwrite(&v, 2, 1, f); }
static void put32(FILE *f, uint32_t v) { fwrite(&v, 4, 1, f); }

int main(void) {
  const int S = 32;
  FILE *f = fopen("icon.ico", "wb");
  if (!f) return 1;
  put16(f, 0); put16(f, 1); put16(f, 1);           /* ICONDIR */
  fputc(S, f); fputc(S, f); fputc(0, f); fputc(0, f); /* ICONDIRENTRY */
  put16(f, 1); put16(f, 32);                       /* planes, bpp */
  const uint32_t xorBytes = (uint32_t)S * S * 4;
  const uint32_t maskBytes = (uint32_t)S * 4;
  put32(f, 40 + xorBytes + maskBytes);             /* size in file */
  put32(f, 22);                                    /* offset */
  put32(f, 40); put32(f, S); put32(f, S * 2);      /* BITMAPINFOHEADER */
  put16(f, 1); put16(f, 32);
  put32(f, 0); put32(f, 0); put32(f, 0); put32(f, 0); put32(f, 0); put32(f, 0);
  for (int y = S - 1; y >= 0; --y) {               /* bottom-up XOR */
    for (int x = 0; x < S; ++x) {
      int inBar = (x >= 9 && x <= 14 && y >= 9 && y <= 22);
      int inBox = (x >= 9 && x <= 22 && y >= 9 && y <= 22);
      int white = inBar || (inBox && (x == 9 || x == 22 || y == 9 || y == 22));
      if (white) { fputc(255, f); fputc(255, f); fputc(255, f); fputc(255, f); }
      else       { fputc(232, f); fputc(95, f); fputc(51, f); fputc(255, f); }
    }
  }
  for (int i = 0; i < S * 4; ++i) fputc(0, f);     /* AND mask all-opaque */
  fclose(f);
  return 0;
}