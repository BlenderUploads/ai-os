#ifndef HALCYON_TIME_H
#define HALCYON_TIME_H
#include <stddef.h>
typedef long time_t;
struct tm { int tm_sec, tm_min, tm_hour, tm_mday, tm_mon, tm_year, tm_wday, tm_yday, tm_isdst; };
time_t time(time_t *store);
struct tm *localtime(const time_t *value);
size_t strftime(char *buffer, size_t size, const char *format, const struct tm *value);
#endif
