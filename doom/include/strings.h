/* <strings.h> — the case-insensitive comparisons, which live here on POSIX. */
#ifndef HALCYON_STRINGS_H
#define HALCYON_STRINGS_H
#include <stddef.h>
int strcasecmp(const char *a, const char *b);
int strncasecmp(const char *a, const char *b, size_t count);
#endif
