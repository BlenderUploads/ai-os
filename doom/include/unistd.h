/* DOOM only wants access(), and only to probe for WAD files. */
#ifndef HALCYON_UNISTD_H
#define HALCYON_UNISTD_H
#include <stddef.h>
#define F_OK 0
#define R_OK 4
#define W_OK 2
#define X_OK 1
int access(const char *path, int mode);
int mkdir(const char *path, int mode);
#endif
