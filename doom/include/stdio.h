/* Minimal <stdio.h> for HALCYON's DOOM port.
 *
 * FILE is a handle into the kernel's RAM filesystem; there is no buffering
 * and no real stdio machinery. Writes to stdout and stderr go to the serial
 * console, which is where the boot log already goes.
 */
#ifndef HALCYON_STDIO_H
#define HALCYON_STDIO_H

#include <stddef.h>
#include <stdarg.h>

typedef struct _HALCYON_FILE FILE;

extern FILE *stdout;
extern FILE *stderr;
extern FILE *stdin;

#define EOF (-1)
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
#define BUFSIZ 1024
#define FILENAME_MAX 256

FILE *fopen(const char *path, const char *mode);
int fclose(FILE *stream);
size_t fread(void *buffer, size_t size, size_t count, FILE *stream);
size_t fwrite(const void *buffer, size_t size, size_t count, FILE *stream);
int fseek(FILE *stream, long offset, int whence);
long ftell(FILE *stream);
void rewind(FILE *stream);
int feof(FILE *stream);
int ferror(FILE *stream);
int fflush(FILE *stream);
int fgetc(FILE *stream);
char *fgets(char *buffer, int size, FILE *stream);
int fputc(int ch, FILE *stream);
int fputs(const char *text, FILE *stream);
int puts(const char *text);
int putchar(int ch);
int remove(const char *path);
int rename(const char *from, const char *to);

int printf(const char *format, ...);
int fprintf(FILE *stream, const char *format, ...);
int sprintf(char *buffer, const char *format, ...);
int snprintf(char *buffer, size_t size, const char *format, ...);
int vsnprintf(char *buffer, size_t size, const char *format, va_list args);
int vfprintf(FILE *stream, const char *format, va_list args);
int vprintf(const char *format, va_list args);
int sscanf(const char *input, const char *format, ...);

#endif
