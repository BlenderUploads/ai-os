#ifndef HALCYON_ASSERT_H
#define HALCYON_ASSERT_H
void halcyon_assert_failed(const char *expression, const char *file, int line);
#define assert(expression) \
    ((expression) ? (void)0 : halcyon_assert_failed(#expression, __FILE__, __LINE__))
#endif
