/*
 * SDL_mixer.h -- deliberately empty.
 *
 * doom/src/i_sound.c includes <SDL_mixer.h> whenever FEATURE_SOUND is defined
 * and the target is not DJGPP. It never calls anything from it: with ORIGCODE
 * undefined, every reference to SDL_mixer in that file is a comment. HALCYON's
 * sound module lives in doom/sound.c instead.
 *
 * This header exists so the vendored engine can stay byte-for-byte unmodified.
 */

#ifndef HALCYON_SDL_MIXER_H
#define HALCYON_SDL_MIXER_H

#endif
