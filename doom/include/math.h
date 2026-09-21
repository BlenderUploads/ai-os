/* The handful of <math.h> entries DOOM actually calls, all during start-up
 * table generation rather than in any inner loop. */
#ifndef HALCYON_MATH_H
#define HALCYON_MATH_H

#define M_PI 3.14159265358979323846

double sin(double x);
double cos(double x);
double tan(double x);
double atan(double x);
double atan2(double y, double x);
double sqrt(double x);
double pow(double base, double exponent);
double fabs(double x);
double floor(double x);
double ceil(double x);
double exp(double x);
double log(double x);

#endif
