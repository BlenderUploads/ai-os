/* Minimal <ctype.h>, ASCII only. */
#ifndef HALCYON_CTYPE_H
#define HALCYON_CTYPE_H

int isalpha(int ch);
int isdigit(int ch);
int isalnum(int ch);
int isspace(int ch);
int isupper(int ch);
int islower(int ch);
int isprint(int ch);
int iscntrl(int ch);
int ispunct(int ch);
int isxdigit(int ch);
int toupper(int ch);
int tolower(int ch);

#endif
