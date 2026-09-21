/* Minimal <string.h>. */
#ifndef HALCYON_STRING_H
#define HALCYON_STRING_H

#include <stddef.h>

void *memcpy(void *destination, const void *source, size_t count);
void *memmove(void *destination, const void *source, size_t count);
void *memset(void *destination, int value, size_t count);
int memcmp(const void *a, const void *b, size_t count);
void *memchr(const void *block, int value, size_t count);

char *strcpy(char *destination, const char *source);
char *strncpy(char *destination, const char *source, size_t count);
char *strcat(char *destination, const char *source);
char *strncat(char *destination, const char *source, size_t count);
int strcmp(const char *a, const char *b);
int strncmp(const char *a, const char *b, size_t count);
int strcasecmp(const char *a, const char *b);
int strncasecmp(const char *a, const char *b, size_t count);
size_t strlen(const char *text);
char *strchr(const char *text, int ch);
char *strrchr(const char *text, int ch);
char *strstr(const char *haystack, const char *needle);
char *strdup(const char *text);
char *strerror(int error);
size_t strspn(const char *text, const char *accept);

#endif
