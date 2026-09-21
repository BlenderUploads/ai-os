/* Minimal <stdlib.h>. Allocation goes straight to the kernel heap. */
#ifndef HALCYON_STDLIB_H
#define HALCYON_STDLIB_H

#include <stddef.h>

#define RAND_MAX 32767

void *malloc(size_t size);
void *calloc(size_t count, size_t size);
void *realloc(void *pointer, size_t size);
void free(void *pointer);

int atoi(const char *text);
long atol(const char *text);
double atof(const char *text);
long strtol(const char *text, char **end, int base);
unsigned long strtoul(const char *text, char **end, int base);
double strtod(const char *text, char **end);

void exit(int status);
void abort(void);
char *getenv(const char *name);
int system(const char *command);
int abs(int value);
void qsort(void *base, size_t count, size_t size,
           int (*compare)(const void *, const void *));
int rand(void);
void srand(unsigned int seed);

#endif
