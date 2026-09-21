/* A C library just large enough to run DOOM on HALCYON.
 *
 * Nothing here is a general-purpose libc. It is the smallest surface that
 * doomgeneric actually calls, implemented against six hooks the kernel
 * exports (see hal_* below). The kernel side lives in kernel/src/apps/doom.rs.
 *
 * Two decisions worth knowing about:
 *
 *   - malloc carries its own size header, because Rust's allocator needs the
 *     layout back at free time and C's free() does not supply one.
 *
 *   - A FILE opened for reading borrows the kernel's bytes rather than copying
 *     them. The IWAD is nearly 30 MB; copying it per open would be absurd.
 */

#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>

#include <ctype.h>
#include <errno.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

/* ---- hooks provided by the kernel ---------------------------------- */

extern void hal_log(const char *text, size_t length);
extern void *hal_alloc(size_t size, size_t align);
extern void hal_release(void *pointer, size_t size, size_t align);
extern const unsigned char *hal_file_open(const char *path, size_t *length);
extern int hal_file_store(const char *path, const void *data, size_t length);
extern int hal_file_exists(const char *path);
extern uint32_t hal_ticks_ms(void);
extern void hal_panic(const char *message);

int errno = 0;

/* ---- allocation ---------------------------------------------------- */

#define ALLOC_HEADER 16

void *malloc(size_t size)
{
    if (size == 0)
    {
        size = 1;
    }
    unsigned char *block = (unsigned char *)hal_alloc(size + ALLOC_HEADER, ALLOC_HEADER);
    if (block == NULL)
    {
        errno = ENOMEM;
        return NULL;
    }
    *(size_t *)block = size;
    return block + ALLOC_HEADER;
}

void free(void *pointer)
{
    if (pointer == NULL)
    {
        return;
    }
    unsigned char *block = (unsigned char *)pointer - ALLOC_HEADER;
    size_t size = *(size_t *)block;
    hal_release(block, size + ALLOC_HEADER, ALLOC_HEADER);
}

void *calloc(size_t count, size_t size)
{
    size_t total = count * size;
    void *block = malloc(total);
    if (block != NULL)
    {
        memset(block, 0, total);
    }
    return block;
}

void *realloc(void *pointer, size_t size)
{
    if (pointer == NULL)
    {
        return malloc(size);
    }
    if (size == 0)
    {
        free(pointer);
        return NULL;
    }
    size_t previous = *(size_t *)((unsigned char *)pointer - ALLOC_HEADER);
    void *replacement = malloc(size);
    if (replacement == NULL)
    {
        return NULL;
    }
    memcpy(replacement, pointer, previous < size ? previous : size);
    free(pointer);
    return replacement;
}

/* ---- memory and strings -------------------------------------------- */

void *memcpy(void *destination, const void *source, size_t count)
{
    unsigned char *to = destination;
    const unsigned char *from = source;
    while (count--)
    {
        *to++ = *from++;
    }
    return destination;
}

void *memmove(void *destination, const void *source, size_t count)
{
    unsigned char *to = destination;
    const unsigned char *from = source;
    if (to == from || count == 0)
    {
        return destination;
    }
    if (to < from)
    {
        while (count--)
        {
            *to++ = *from++;
        }
    }
    else
    {
        to += count;
        from += count;
        while (count--)
        {
            *--to = *--from;
        }
    }
    return destination;
}

void *memset(void *destination, int value, size_t count)
{
    unsigned char *to = destination;
    while (count--)
    {
        *to++ = (unsigned char)value;
    }
    return destination;
}

int memcmp(const void *a, const void *b, size_t count)
{
    const unsigned char *left = a;
    const unsigned char *right = b;
    while (count--)
    {
        if (*left != *right)
        {
            return (int)*left - (int)*right;
        }
        left++;
        right++;
    }
    return 0;
}

void *memchr(const void *block, int value, size_t count)
{
    const unsigned char *at = block;
    while (count--)
    {
        if (*at == (unsigned char)value)
        {
            return (void *)at;
        }
        at++;
    }
    return NULL;
}

size_t strlen(const char *text)
{
    size_t length = 0;
    while (text[length] != '\0')
    {
        length++;
    }
    return length;
}

char *strcpy(char *destination, const char *source)
{
    char *start = destination;
    while ((*destination++ = *source++) != '\0')
    {
    }
    return start;
}

char *strncpy(char *destination, const char *source, size_t count)
{
    size_t index = 0;
    for (; index < count && source[index] != '\0'; index++)
    {
        destination[index] = source[index];
    }
    for (; index < count; index++)
    {
        destination[index] = '\0';
    }
    return destination;
}

char *strcat(char *destination, const char *source)
{
    strcpy(destination + strlen(destination), source);
    return destination;
}

char *strncat(char *destination, const char *source, size_t count)
{
    char *end = destination + strlen(destination);
    while (count-- && *source != '\0')
    {
        *end++ = *source++;
    }
    *end = '\0';
    return destination;
}

int strcmp(const char *a, const char *b)
{
    while (*a != '\0' && *a == *b)
    {
        a++;
        b++;
    }
    return (int)(unsigned char)*a - (int)(unsigned char)*b;
}

int strncmp(const char *a, const char *b, size_t count)
{
    while (count-- > 0)
    {
        if (*a != *b)
        {
            return (int)(unsigned char)*a - (int)(unsigned char)*b;
        }
        if (*a == '\0')
        {
            return 0;
        }
        a++;
        b++;
    }
    return 0;
}

int strcasecmp(const char *a, const char *b)
{
    while (*a != '\0' && tolower((unsigned char)*a) == tolower((unsigned char)*b))
    {
        a++;
        b++;
    }
    return tolower((unsigned char)*a) - tolower((unsigned char)*b);
}

int strncasecmp(const char *a, const char *b, size_t count)
{
    while (count-- > 0)
    {
        int left = tolower((unsigned char)*a);
        int right = tolower((unsigned char)*b);
        if (left != right)
        {
            return left - right;
        }
        if (left == 0)
        {
            return 0;
        }
        a++;
        b++;
    }
    return 0;
}

char *strchr(const char *text, int ch)
{
    for (;; text++)
    {
        if (*text == (char)ch)
        {
            return (char *)text;
        }
        if (*text == '\0')
        {
            return NULL;
        }
    }
}

char *strrchr(const char *text, int ch)
{
    const char *found = NULL;
    for (;; text++)
    {
        if (*text == (char)ch)
        {
            found = text;
        }
        if (*text == '\0')
        {
            return (char *)found;
        }
    }
}

char *strstr(const char *haystack, const char *needle)
{
    size_t length = strlen(needle);
    if (length == 0)
    {
        return (char *)haystack;
    }
    for (; *haystack != '\0'; haystack++)
    {
        if (strncmp(haystack, needle, length) == 0)
        {
            return (char *)haystack;
        }
    }
    return NULL;
}

size_t strspn(const char *text, const char *accept)
{
    size_t count = 0;
    while (text[count] != '\0' && strchr(accept, text[count]) != NULL)
    {
        count++;
    }
    return count;
}

char *strdup(const char *text)
{
    size_t length = strlen(text) + 1;
    char *copy = malloc(length);
    if (copy != NULL)
    {
        memcpy(copy, text, length);
    }
    return copy;
}

char *strerror(int error)
{
    (void)error;
    return "error";
}

/* ---- character classes --------------------------------------------- */

int isdigit(int ch) { return ch >= '0' && ch <= '9'; }
int isupper(int ch) { return ch >= 'A' && ch <= 'Z'; }
int islower(int ch) { return ch >= 'a' && ch <= 'z'; }
int isalpha(int ch) { return isupper(ch) || islower(ch); }
int isalnum(int ch) { return isalpha(ch) || isdigit(ch); }
int isspace(int ch)
{
    return ch == ' ' || ch == '\t' || ch == '\n' || ch == '\v' || ch == '\f' || ch == '\r';
}
int isprint(int ch) { return ch >= 0x20 && ch < 0x7F; }
int iscntrl(int ch) { return ch < 0x20 || ch == 0x7F; }
int ispunct(int ch) { return isprint(ch) && !isalnum(ch) && ch != ' '; }
int isxdigit(int ch)
{
    return isdigit(ch) || (ch >= 'a' && ch <= 'f') || (ch >= 'A' && ch <= 'F');
}
int toupper(int ch) { return islower(ch) ? ch - 'a' + 'A' : ch; }
int tolower(int ch) { return isupper(ch) ? ch - 'A' + 'a' : ch; }

/* ---- numbers ------------------------------------------------------- */

int abs(int value) { return value < 0 ? -value : value; }

long strtol(const char *text, char **end, int base)
{
    const char *at = text;
    while (isspace((unsigned char)*at))
    {
        at++;
    }
    int negative = 0;
    if (*at == '+' || *at == '-')
    {
        negative = (*at == '-');
        at++;
    }
    if ((base == 0 || base == 16) && at[0] == '0' && (at[1] == 'x' || at[1] == 'X'))
    {
        at += 2;
        base = 16;
    }
    else if (base == 0)
    {
        base = (at[0] == '0') ? 8 : 10;
    }

    long value = 0;
    for (;; at++)
    {
        int digit;
        if (isdigit((unsigned char)*at))
        {
            digit = *at - '0';
        }
        else if (isalpha((unsigned char)*at))
        {
            digit = tolower((unsigned char)*at) - 'a' + 10;
        }
        else
        {
            break;
        }
        if (digit >= base)
        {
            break;
        }
        value = value * base + digit;
    }
    if (end != NULL)
    {
        *end = (char *)at;
    }
    return negative ? -value : value;
}

unsigned long strtoul(const char *text, char **end, int base)
{
    return (unsigned long)strtol(text, end, base);
}

int atoi(const char *text) { return (int)strtol(text, NULL, 10); }
long atol(const char *text) { return strtol(text, NULL, 10); }

double strtod(const char *text, char **end)
{
    const char *at = text;
    while (isspace((unsigned char)*at))
    {
        at++;
    }
    int negative = 0;
    if (*at == '+' || *at == '-')
    {
        negative = (*at == '-');
        at++;
    }
    double value = 0.0;
    while (isdigit((unsigned char)*at))
    {
        value = value * 10.0 + (*at++ - '0');
    }
    if (*at == '.')
    {
        at++;
        double scale = 0.1;
        while (isdigit((unsigned char)*at))
        {
            value += (*at++ - '0') * scale;
            scale *= 0.1;
        }
    }
    if (end != NULL)
    {
        *end = (char *)at;
    }
    return negative ? -value : value;
}

double atof(const char *text) { return strtod(text, NULL); }

static unsigned long rng_state = 1;

int rand(void)
{
    rng_state = rng_state * 1103515245UL + 12345UL;
    return (int)((rng_state >> 16) & 0x7FFF);
}

void srand(unsigned int seed) { rng_state = seed; }

void qsort(void *base, size_t count, size_t size,
           int (*compare)(const void *, const void *))
{
    /* Insertion sort: DOOM only sorts tiny arrays, and this cannot blow the
     * stack the way an unlucky quicksort partition could. */
    unsigned char *items = base;
    for (size_t i = 1; i < count; i++)
    {
        for (size_t j = i; j > 0; j--)
        {
            unsigned char *left = items + (j - 1) * size;
            unsigned char *right = items + j * size;
            if (compare(left, right) <= 0)
            {
                break;
            }
            for (size_t byte = 0; byte < size; byte++)
            {
                unsigned char swap = left[byte];
                left[byte] = right[byte];
                right[byte] = swap;
            }
        }
    }
}

/* ---- maths ---------------------------------------------------------
 *
 * Only called while DOOM builds its lookup tables at start-up, so these
 * favour being short and obviously correct over being fast.
 */

double fabs(double x) { return x < 0.0 ? -x : x; }

double floor(double x)
{
    double truncated = (double)(long long)x;
    return (x < 0.0 && truncated != x) ? truncated - 1.0 : truncated;
}

double ceil(double x)
{
    double truncated = (double)(long long)x;
    return (x > 0.0 && truncated != x) ? truncated + 1.0 : truncated;
}

double sqrt(double x)
{
    if (x <= 0.0)
    {
        return 0.0;
    }
    /* Newton-Raphson from a decent starting guess converges in a few steps. */
    double guess = x > 1.0 ? x : 1.0;
    for (int step = 0; step < 40; step++)
    {
        double next = 0.5 * (guess + x / guess);
        if (fabs(next - guess) < 1e-12 * next)
        {
            return next;
        }
        guess = next;
    }
    return guess;
}

double sin(double x)
{
    /* Fold into [-pi, pi] first, or the Taylor series loses all accuracy. */
    while (x > M_PI)
    {
        x -= 2.0 * M_PI;
    }
    while (x < -M_PI)
    {
        x += 2.0 * M_PI;
    }
    double term = x;
    double total = x;
    for (int n = 1; n < 12; n++)
    {
        term *= -(x * x) / (double)((2 * n) * (2 * n + 1));
        total += term;
    }
    return total;
}

double cos(double x) { return sin(x + M_PI / 2.0); }

double tan(double x)
{
    double c = cos(x);
    if (fabs(c) < 1e-12)
    {
        return x > 0.0 ? 1e12 : -1e12;
    }
    return sin(x) / c;
}

double atan(double x)
{
    int negative = x < 0.0;
    if (negative)
    {
        x = -x;
    }
    /* atan(x) = pi/2 - atan(1/x) keeps the series inside its useful range. */
    int inverted = x > 1.0;
    if (inverted)
    {
        x = 1.0 / x;
    }
    double term = x;
    double total = x;
    for (int n = 1; n < 60; n++)
    {
        term *= -(x * x);
        total += term / (double)(2 * n + 1);
    }
    if (inverted)
    {
        total = M_PI / 2.0 - total;
    }
    return negative ? -total : total;
}

double atan2(double y, double x)
{
    if (x > 0.0)
    {
        return atan(y / x);
    }
    if (x < 0.0)
    {
        return y >= 0.0 ? atan(y / x) + M_PI : atan(y / x) - M_PI;
    }
    if (y > 0.0)
    {
        return M_PI / 2.0;
    }
    if (y < 0.0)
    {
        return -M_PI / 2.0;
    }
    return 0.0;
}

double exp(double x)
{
    double term = 1.0;
    double total = 1.0;
    for (int n = 1; n < 30; n++)
    {
        term *= x / (double)n;
        total += term;
    }
    return total;
}

double log(double x)
{
    if (x <= 0.0)
    {
        return -1e18;
    }
    /* Newton on exp(y) = x. */
    double guess = 0.0;
    for (int step = 0; step < 60; step++)
    {
        double e = exp(guess);
        double next = guess + (x - e) / e;
        if (fabs(next - guess) < 1e-12)
        {
            return next;
        }
        guess = next;
    }
    return guess;
}

double pow(double base, double exponent)
{
    if (base == 0.0)
    {
        return 0.0;
    }
    /* Integer exponents are exact and are all DOOM ever asks for. */
    if (exponent == (double)(long long)exponent && fabs(exponent) < 64.0)
    {
        long long times = (long long)exponent;
        int negative = times < 0;
        if (negative)
        {
            times = -times;
        }
        double result = 1.0;
        while (times--)
        {
            result *= base;
        }
        return negative ? 1.0 / result : result;
    }
    return exp(exponent * log(base));
}

/* ---- formatted output ---------------------------------------------- */

struct sink
{
    char *buffer;
    size_t capacity;
    size_t written;
};

static void sink_char(struct sink *out, char ch)
{
    if (out->buffer != NULL && out->written + 1 < out->capacity)
    {
        out->buffer[out->written] = ch;
    }
    out->written++;
}

static void sink_text(struct sink *out, const char *text, int width, int left, int precision)
{
    size_t length = 0;
    while (text[length] != '\0' && (precision < 0 || (int)length < precision))
    {
        length++;
    }
    int padding = width - (int)length;
    if (!left)
    {
        while (padding-- > 0)
        {
            sink_char(out, ' ');
        }
    }
    for (size_t index = 0; index < length; index++)
    {
        sink_char(out, text[index]);
    }
    if (left)
    {
        while (padding-- > 0)
        {
            sink_char(out, ' ');
        }
    }
}

static void sink_number(struct sink *out, unsigned long long value, int base, int uppercase,
                        int negative, int width, int left, int zero, int precision)
{
    char digits[32];
    int count = 0;
    const char *alphabet = uppercase ? "0123456789ABCDEF" : "0123456789abcdef";
    if (value == 0)
    {
        digits[count++] = '0';
    }
    while (value != 0)
    {
        digits[count++] = alphabet[value % (unsigned)base];
        value /= (unsigned)base;
    }
    while (count < precision && count < (int)sizeof(digits))
    {
        digits[count++] = '0';
    }
    if (negative)
    {
        digits[count++] = '-';
    }

    int padding = width - count;
    if (!left && !zero)
    {
        while (padding-- > 0)
        {
            sink_char(out, ' ');
        }
    }
    if (!left && zero)
    {
        /* A sign has to come before the zero padding, not after it. */
        if (negative)
        {
            sink_char(out, digits[--count]);
        }
        while (padding-- > 0)
        {
            sink_char(out, '0');
        }
    }
    while (count-- > 0)
    {
        sink_char(out, digits[count]);
    }
    if (left)
    {
        while (padding-- > 0)
        {
            sink_char(out, ' ');
        }
    }
}

static int format(struct sink *out, const char *specification, va_list args)
{
    for (const char *at = specification; *at != '\0'; at++)
    {
        if (*at != '%')
        {
            sink_char(out, *at);
            continue;
        }
        at++;
        if (*at == '%')
        {
            sink_char(out, '%');
            continue;
        }

        int left = 0;
        int zero = 0;
        int plus = 0;
        for (;; at++)
        {
            if (*at == '-')
            {
                left = 1;
            }
            else if (*at == '0')
            {
                zero = 1;
            }
            else if (*at == '+' || *at == ' ')
            {
                plus = 1;
            }
            else
            {
                break;
            }
        }
        (void)plus;

        int width = 0;
        if (*at == '*')
        {
            width = va_arg(args, int);
            if (width < 0)
            {
                left = 1;
                width = -width;
            }
            at++;
        }
        else
        {
            while (isdigit((unsigned char)*at))
            {
                width = width * 10 + (*at++ - '0');
            }
        }

        int precision = -1;
        if (*at == '.')
        {
            at++;
            precision = 0;
            if (*at == '*')
            {
                precision = va_arg(args, int);
                at++;
            }
            else
            {
                while (isdigit((unsigned char)*at))
                {
                    precision = precision * 10 + (*at++ - '0');
                }
            }
        }

        int is_long = 0;
        while (*at == 'l' || *at == 'h' || *at == 'z' || *at == 'j' || *at == 't')
        {
            if (*at == 'l' || *at == 'z' || *at == 'j' || *at == 't')
            {
                is_long = 1;
            }
            at++;
        }

        switch (*at)
        {
        case 'd':
        case 'i':
        {
            long long value = is_long ? va_arg(args, long) : va_arg(args, int);
            int negative = value < 0;
            unsigned long long magnitude =
                negative ? (unsigned long long)(-(value + 1)) + 1ULL : (unsigned long long)value;
            sink_number(out, magnitude, 10, 0, negative, width, left, zero, precision);
            break;
        }
        case 'u':
        {
            unsigned long long value =
                is_long ? va_arg(args, unsigned long) : va_arg(args, unsigned int);
            sink_number(out, value, 10, 0, 0, width, left, zero, precision);
            break;
        }
        case 'x':
        case 'X':
        {
            unsigned long long value =
                is_long ? va_arg(args, unsigned long) : va_arg(args, unsigned int);
            sink_number(out, value, 16, *at == 'X', 0, width, left, zero, precision);
            break;
        }
        case 'o':
        {
            unsigned long long value =
                is_long ? va_arg(args, unsigned long) : va_arg(args, unsigned int);
            sink_number(out, value, 8, 0, 0, width, left, zero, precision);
            break;
        }
        case 'p':
        {
            void *value = va_arg(args, void *);
            sink_char(out, '0');
            sink_char(out, 'x');
            sink_number(out, (unsigned long long)(uintptr_t)value, 16, 0, 0, 0, 0, 0, -1);
            break;
        }
        case 'c':
        {
            char value = (char)va_arg(args, int);
            char text[2] = {value, '\0'};
            sink_text(out, text, width, left, -1);
            break;
        }
        case 's':
        {
            const char *value = va_arg(args, const char *);
            sink_text(out, value != NULL ? value : "(null)", width, left, precision);
            break;
        }
        case 'f':
        case 'g':
        case 'e':
        {
            double value = va_arg(args, double);
            int negative = value < 0.0;
            if (negative)
            {
                value = -value;
            }
            unsigned long long whole = (unsigned long long)value;
            double fraction = value - (double)whole;
            sink_number(out, whole, 10, 0, negative, width, left, zero, -1);
            sink_char(out, '.');
            int places = precision < 0 ? 6 : precision;
            for (int index = 0; index < places; index++)
            {
                fraction *= 10.0;
                int digit = (int)fraction;
                sink_char(out, (char)('0' + digit));
                fraction -= digit;
            }
            break;
        }
        default:
            sink_char(out, '%');
            sink_char(out, *at);
            break;
        }
    }

    if (out->buffer != NULL && out->capacity > 0)
    {
        size_t terminator = out->written < out->capacity ? out->written : out->capacity - 1;
        out->buffer[terminator] = '\0';
    }
    return (int)out->written;
}

int vsnprintf(char *buffer, size_t size, const char *specification, va_list args)
{
    struct sink out = {buffer, size, 0};
    return format(&out, specification, args);
}

int snprintf(char *buffer, size_t size, const char *specification, ...)
{
    va_list args;
    va_start(args, specification);
    int written = vsnprintf(buffer, size, specification, args);
    va_end(args);
    return written;
}

int sprintf(char *buffer, const char *specification, ...)
{
    va_list args;
    va_start(args, specification);
    int written = vsnprintf(buffer, (size_t)1 << 30, specification, args);
    va_end(args);
    return written;
}

/* Everything printed lands on the serial console. */
static void emit(const char *specification, va_list args)
{
    char line[1024];
    struct sink out = {line, sizeof(line), 0};
    format(&out, specification, args);
    size_t length = out.written < sizeof(line) - 1 ? out.written : sizeof(line) - 1;
    hal_log(line, length);
}

int printf(const char *specification, ...)
{
    va_list args;
    va_start(args, specification);
    emit(specification, args);
    va_end(args);
    return 0;
}

int vprintf(const char *specification, va_list args)
{
    emit(specification, args);
    return 0;
}

int fprintf(FILE *stream, const char *specification, ...)
{
    (void)stream;
    va_list args;
    va_start(args, specification);
    emit(specification, args);
    va_end(args);
    return 0;
}

int vfprintf(FILE *stream, const char *specification, va_list args)
{
    (void)stream;
    emit(specification, args);
    return 0;
}

int puts(const char *text)
{
    hal_log(text, strlen(text));
    hal_log("\n", 1);
    return 0;
}

int putchar(int ch)
{
    char value = (char)ch;
    hal_log(&value, 1);
    return ch;
}

int sscanf(const char *input, const char *specification, ...)
{
    /* DOOM uses this only for simple integer parsing in the config reader. */
    va_list args;
    va_start(args, specification);
    int matched = 0;
    const char *at = input;
    for (const char *spec = specification; *spec != '\0'; spec++)
    {
        if (*spec != '%')
        {
            continue;
        }
        spec++;
        while (isdigit((unsigned char)*spec) || *spec == 'l')
        {
            spec++;
        }
        while (isspace((unsigned char)*at))
        {
            at++;
        }
        if (*spec == 'd' || *spec == 'i')
        {
            char *end = NULL;
            long value = strtol(at, &end, 10);
            if (end == at)
            {
                break;
            }
            *va_arg(args, int *) = (int)value;
            at = end;
            matched++;
        }
        else if (*spec == 'x')
        {
            char *end = NULL;
            long value = strtol(at, &end, 16);
            if (end == at)
            {
                break;
            }
            *va_arg(args, int *) = (int)value;
            at = end;
            matched++;
        }
        else if (*spec == 's')
        {
            char *target = va_arg(args, char *);
            while (*at != '\0' && !isspace((unsigned char)*at))
            {
                *target++ = *at++;
            }
            *target = '\0';
            matched++;
        }
        else
        {
            break;
        }
    }
    va_end(args);
    return matched;
}

/* ---- files ---------------------------------------------------------- */

struct _HALCYON_FILE
{
    const unsigned char *data; /* borrowed from the kernel for reads */
    size_t length;
    size_t position;
    int writable;
    int at_end;
    unsigned char *pending; /* accumulated writes, flushed on close */
    size_t pending_length;
    size_t pending_capacity;
    char path[256];
};

static FILE console_out;
static FILE console_err;
static FILE console_in;
FILE *stdout = &console_out;
FILE *stderr = &console_err;
FILE *stdin = &console_in;

FILE *fopen(const char *path, const char *mode)
{
    int writable = (strchr(mode, 'w') != NULL || strchr(mode, 'a') != NULL);
    size_t length = 0;
    const unsigned char *data = writable ? NULL : hal_file_open(path, &length);
    if (!writable && data == NULL)
    {
        errno = ENOENT;
        return NULL;
    }

    FILE *stream = malloc(sizeof(FILE));
    if (stream == NULL)
    {
        return NULL;
    }
    memset(stream, 0, sizeof(FILE));
    stream->data = data;
    stream->length = length;
    stream->writable = writable;
    strncpy(stream->path, path, sizeof(stream->path) - 1);
    return stream;
}

static int grow_pending(FILE *stream, size_t needed)
{
    if (stream->pending_length + needed <= stream->pending_capacity)
    {
        return 1;
    }
    size_t capacity = stream->pending_capacity ? stream->pending_capacity : 1024;
    while (capacity < stream->pending_length + needed)
    {
        capacity *= 2;
    }
    unsigned char *grown = realloc(stream->pending, capacity);
    if (grown == NULL)
    {
        return 0;
    }
    stream->pending = grown;
    stream->pending_capacity = capacity;
    return 1;
}

size_t fwrite(const void *buffer, size_t size, size_t count, FILE *stream)
{
    if (stream == NULL || !stream->writable)
    {
        /* Writing to the console streams: just log it. */
        if (stream == stdout || stream == stderr)
        {
            hal_log(buffer, size * count);
            return count;
        }
        return 0;
    }
    size_t bytes = size * count;
    if (!grow_pending(stream, bytes))
    {
        return 0;
    }
    memcpy(stream->pending + stream->pending_length, buffer, bytes);
    stream->pending_length += bytes;
    return count;
}

size_t fread(void *buffer, size_t size, size_t count, FILE *stream)
{
    if (stream == NULL || stream->data == NULL)
    {
        return 0;
    }
    size_t wanted = size * count;
    size_t available = stream->length - stream->position;
    if (wanted > available)
    {
        wanted = available;
        stream->at_end = 1;
    }
    memcpy(buffer, stream->data + stream->position, wanted);
    stream->position += wanted;
    return size == 0 ? 0 : wanted / size;
}

int fseek(FILE *stream, long offset, int whence)
{
    if (stream == NULL)
    {
        return -1;
    }
    long base;
    switch (whence)
    {
    case SEEK_SET:
        base = 0;
        break;
    case SEEK_CUR:
        base = (long)stream->position;
        break;
    case SEEK_END:
        base = (long)(stream->writable ? stream->pending_length : stream->length);
        break;
    default:
        return -1;
    }
    long target = base + offset;
    if (target < 0)
    {
        return -1;
    }
    stream->position = (size_t)target;
    stream->at_end = 0;
    return 0;
}

long ftell(FILE *stream) { return stream == NULL ? -1 : (long)stream->position; }

void rewind(FILE *stream)
{
    if (stream != NULL)
    {
        stream->position = 0;
        stream->at_end = 0;
    }
}

int feof(FILE *stream)
{
    return stream != NULL && (stream->at_end || stream->position >= stream->length);
}

int ferror(FILE *stream)
{
    (void)stream;
    return 0;
}

int fflush(FILE *stream)
{
    (void)stream;
    return 0;
}

int fclose(FILE *stream)
{
    if (stream == NULL)
    {
        return -1;
    }
    if (stream->writable && stream->pending != NULL)
    {
        hal_file_store(stream->path, stream->pending, stream->pending_length);
        free(stream->pending);
    }
    free(stream);
    return 0;
}

int fgetc(FILE *stream)
{
    if (stream == NULL || stream->data == NULL || stream->position >= stream->length)
    {
        return EOF;
    }
    return stream->data[stream->position++];
}

char *fgets(char *buffer, int size, FILE *stream)
{
    if (size <= 0 || stream == NULL)
    {
        return NULL;
    }
    int index = 0;
    while (index < size - 1)
    {
        int ch = fgetc(stream);
        if (ch == EOF)
        {
            break;
        }
        buffer[index++] = (char)ch;
        if (ch == '\n')
        {
            break;
        }
    }
    if (index == 0)
    {
        return NULL;
    }
    buffer[index] = '\0';
    return buffer;
}

int fputc(int ch, FILE *stream)
{
    char value = (char)ch;
    fwrite(&value, 1, 1, stream);
    return ch;
}

int fputs(const char *text, FILE *stream)
{
    fwrite(text, 1, strlen(text), stream);
    return 0;
}

int remove(const char *path)
{
    (void)path;
    return 0;
}

int rename(const char *from, const char *to)
{
    (void)from;
    (void)to;
    return 0;
}

int access(const char *path, int mode)
{
    (void)mode;
    return hal_file_exists(path) ? 0 : -1;
}

int mkdir(const char *path, int mode)
{
    (void)path;
    (void)mode;
    return 0;
}

/* ---- odds and ends --------------------------------------------------- */

char *getenv(const char *name)
{
    (void)name;
    return NULL;
}

int system(const char *command)
{
    (void)command;
    return -1;
}

time_t time(time_t *store)
{
    time_t value = (time_t)(hal_ticks_ms() / 1000);
    if (store != NULL)
    {
        *store = value;
    }
    return value;
}

struct tm *localtime(const time_t *value)
{
    static struct tm broken_down;
    long seconds = value != NULL ? (long)*value : 0;
    broken_down.tm_sec = (int)(seconds % 60);
    broken_down.tm_min = (int)((seconds / 60) % 60);
    broken_down.tm_hour = (int)((seconds / 3600) % 24);
    broken_down.tm_mday = 1;
    broken_down.tm_mon = 0;
    broken_down.tm_year = 126; /* 2026 */
    return &broken_down;
}

size_t strftime(char *buffer, size_t size, const char *specification, const struct tm *value)
{
    (void)specification;
    (void)value;
    if (size > 0)
    {
        buffer[0] = '\0';
    }
    return 0;
}

void exit(int status)
{
    char message[64];
    snprintf(message, sizeof(message), "DOOM called exit(%d)", status);
    hal_panic(message);
    for (;;)
    {
    }
}

void abort(void)
{
    hal_panic("DOOM called abort()");
    for (;;)
    {
    }
}

void halcyon_assert_failed(const char *expression, const char *file, int line)
{
    char message[256];
    snprintf(message, sizeof(message), "assertion failed: %s at %s:%d", expression, file, line);
    hal_panic(message);
    for (;;)
    {
    }
}
