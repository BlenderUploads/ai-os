//
// HALCYON's sound backend for doomgeneric.
//
// Copyright(C) 1993-1996 Id Software, Inc.
// Copyright(C) 2005-2014 Simon Howard
//
// This program is free software; you can redistribute it and/or
// modify it under the terms of the GNU General Public License
// as published by the Free Software Foundation; either version 2
// of the License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// DESCRIPTION:
//     doomgeneric leaves DG_sound_module and DG_music_module for the platform
//     to define. This is HALCYON's: an eight-voice mixer that resamples DMX
//     sound lumps to whatever rate the kernel's audio device runs at, and
//     hands the result to the AC'97 driver through four hal_ hooks.
//
//     Nothing here is pulled in unless FEATURE_SOUND is defined, which the
//     build only does for the DOOM ISO. It is not part of the vendored engine
//     and lives outside doom/src/ so that directory stays unmodified, but it
//     links into the engine and so is GPL-2 all the same.
//

#include <stdlib.h>
#include <string.h>

#include "doomtype.h"
#include "i_sound.h"
#include "m_misc.h"
#include "deh_str.h"
#include "w_wad.h"

//
// The kernel side. These live in kernel/src/apps/doom.rs.
//

// Output rate in Hz, or zero when the machine has no audio device.
extern unsigned int hal_audio_rate(void);
// Stereo frames already queued for playback.
extern unsigned int hal_audio_queued(void);
// Queue interleaved stereo frames; returns how many were accepted.
extern unsigned int hal_audio_write(const short *frames, unsigned int count);

// Voices. DOOM itself only ever uses snd_channels (8 by default), but the
// engine is free to raise that and a voice costs almost nothing.
#define NUM_CHANNELS 16

// How much audio to keep queued: about 64 ms. The AC'97 driver holds roughly
// as much again, so a sound is heard within about an eighth of a second even
// though I_UpdateSound is only called once per frame.
#define QUEUE_TARGET_FRAMES 3072

// The most to mix in one pass, which bounds the scratch buffers below.
#define MIX_FRAMES 1024

// A DMX sound lump: u16 format (always 3), u16 sample rate, u32 length, then
// the samples as unsigned 8-bit, padded with 16 bytes at each end.
#define DMX_HEADER_BYTES 24
#define DMX_PADDING_BYTES 32

typedef struct
{
    const unsigned char *samples;
    unsigned int length;
    unsigned int rate;
    // The lump buffer the samples point into, kept so it can be freed.
    unsigned char *lump;
} cachedsound_t;

typedef struct
{
    const unsigned char *samples;
    unsigned int length;
    // Playback position and step, both 16.16 fixed point, so an 11 kHz lump
    // can be read out at 48 kHz without a division per sample.
    unsigned int position;
    unsigned int step;
    int left;
    int right;
    boolean playing;
} voice_t;

// Configuration variables the engine binds when the sound path is compiled in.
// Upstream they belong to the SDL backend's libsamplerate resampler; HALCYON's
// mixer resamples itself, so they exist only to be read out of a config file
// and ignored.
int use_libsamplerate = 0;
float libsamplerate_scale = 0.65f;

static voice_t voices[NUM_CHANNELS];
static boolean sound_ready = false;
static boolean sfx_prefix = true;
static unsigned int output_rate = 0;

static int accumulator[MIX_FRAMES * 2];
static short mixbuffer[MIX_FRAMES * 2];

//
// Loading
//

// Read one sound lump into memory we own, rather than the zone: the zone is
// 6 MiB and a full set of sound effects would crowd out the level. Each lump
// is loaded once, on its first use, and kept for as long as the game runs --
// a full set is a couple of megabytes, and HALCYON's heap has room.
static cachedsound_t *CacheSound(sfxinfo_t *sfxinfo)
{
    cachedsound_t *cached;
    unsigned char *lump;
    unsigned int lumplen;
    unsigned int rate;
    unsigned int length;

    cached = (cachedsound_t *) sfxinfo->driver_data;

    if (cached != NULL)
    {
        return cached;
    }

    if (sfxinfo->lumpnum < 0)
    {
        return NULL;
    }

    lumplen = W_LumpLength(sfxinfo->lumpnum);

    if (lumplen <= DMX_HEADER_BYTES + DMX_PADDING_BYTES)
    {
        return NULL;
    }

    lump = (unsigned char *) malloc(lumplen);

    if (lump == NULL)
    {
        return NULL;
    }

    W_ReadLump(sfxinfo->lumpnum, lump);

    // Anything that is not a DMX effect (an Ogg or MP3 replacement, say) is
    // simply not played; guessing at the format would be worse.
    if (lump[0] != 0x03 || lump[1] != 0x00)
    {
        free(lump);
        return NULL;
    }

    rate = (unsigned int) lump[2] | ((unsigned int) lump[3] << 8);
    length = (unsigned int) lump[4] | ((unsigned int) lump[5] << 8)
           | ((unsigned int) lump[6] << 16) | ((unsigned int) lump[7] << 24);

    if (rate == 0 || length <= DMX_PADDING_BYTES)
    {
        free(lump);
        return NULL;
    }

    length -= DMX_PADDING_BYTES;

    // Trust the lump's own size over its header, which some WADs get wrong.
    if (DMX_HEADER_BYTES + length > lumplen)
    {
        length = lumplen - DMX_HEADER_BYTES;
    }

    cached = (cachedsound_t *) malloc(sizeof(cachedsound_t));

    if (cached == NULL)
    {
        free(lump);
        return NULL;
    }

    cached->lump = lump;
    cached->samples = lump + DMX_HEADER_BYTES;
    cached->length = length;
    cached->rate = rate;

    sfxinfo->driver_data = cached;

    return cached;
}

//
// Mixing
//

// Vanilla DOOM's stereo law, kept because it is what the game was balanced
// against: sep runs 0 (hard left) to 254 (hard right), vol 0 to 127.
static void SetVoiceVolume(voice_t *voice, int vol, int sep)
{
    int near;

    if (vol < 0)
    {
        vol = 0;
    }
    else if (vol > 127)
    {
        vol = 127;
    }

    if (sep < 0)
    {
        sep = 0;
    }
    else if (sep > 254)
    {
        sep = 254;
    }

    near = sep + 1;
    voice->left = vol - ((vol * near * near) >> 16);
    near = 257 - near;
    voice->right = vol - ((vol * near * near) >> 16);
}

static void MixVoice(voice_t *voice, int frames)
{
    int frame;

    for (frame = 0; frame < frames; ++frame)
    {
        unsigned int index = voice->position >> 16;
        int sample;

        if (index >= voice->length)
        {
            voice->playing = false;
            return;
        }

        // DMX samples are unsigned; centre them.
        sample = (int) voice->samples[index] - 128;

        accumulator[frame * 2] += sample * voice->left;
        accumulator[frame * 2 + 1] += sample * voice->right;

        voice->position += voice->step;
    }
}

static void MixAndQueue(unsigned int frames)
{
    unsigned int sample;
    int channel;

    memset(accumulator, 0, sizeof(int) * frames * 2);

    for (channel = 0; channel < NUM_CHANNELS; ++channel)
    {
        if (voices[channel].playing)
        {
            MixVoice(&voices[channel], (int) frames);
        }
    }

    // A single voice at full volume peaks around half of full scale, which
    // leaves room for a few more before anything clips.
    for (sample = 0; sample < frames * 2; ++sample)
    {
        int value = accumulator[sample];

        if (value > 32767)
        {
            value = 32767;
        }
        else if (value < -32768)
        {
            value = -32768;
        }

        mixbuffer[sample] = (short) value;
    }

    hal_audio_write(mixbuffer, frames);
}

//
// The module
//

static snddevice_t sound_devices[] =
{
    SNDDEVICE_SB,
    SNDDEVICE_PAS,
    SNDDEVICE_GUS,
    SNDDEVICE_WAVEBLASTER,
    SNDDEVICE_SOUNDCANVAS,
    SNDDEVICE_AWE32,
};

static boolean HAL_InitSound(boolean use_sfx_prefix)
{
    output_rate = hal_audio_rate();

    if (output_rate == 0)
    {
        // No audio device. Returning false leaves DOOM's sound_module NULL,
        // and the game runs silently rather than pretending.
        return false;
    }

    sfx_prefix = use_sfx_prefix;
    memset(voices, 0, sizeof(voices));
    sound_ready = true;

    return true;
}

static void HAL_ShutdownSound(void)
{
    sound_ready = false;
    memset(voices, 0, sizeof(voices));
}

static int HAL_GetSfxLumpNum(sfxinfo_t *sfxinfo)
{
    char name[9];

    if (sfx_prefix)
    {
        M_snprintf(name, sizeof(name), "ds%s", DEH_String(sfxinfo->name));
    }
    else
    {
        M_StringCopy(name, DEH_String(sfxinfo->name), sizeof(name));
    }

    return W_GetNumForName(name);
}

// Called once a frame from S_UpdateSounds. Tops the kernel's queue back up to
// QUEUE_TARGET_FRAMES, which self-clocks: the queue only drains as fast as the
// codec plays it, so this cannot run ahead of real time.
static void HAL_UpdateSound(void)
{
    unsigned int queued;
    unsigned int wanted;

    if (!sound_ready)
    {
        return;
    }

    queued = hal_audio_queued();

    if (queued >= QUEUE_TARGET_FRAMES)
    {
        return;
    }

    wanted = QUEUE_TARGET_FRAMES - queued;

    while (wanted > 0)
    {
        unsigned int frames = wanted > MIX_FRAMES ? MIX_FRAMES : wanted;

        MixAndQueue(frames);
        wanted -= frames;
    }
}

static void HAL_UpdateSoundParams(int channel, int vol, int sep)
{
    if (!sound_ready || channel < 0 || channel >= NUM_CHANNELS)
    {
        return;
    }

    SetVoiceVolume(&voices[channel], vol, sep);
}

static int HAL_StartSound(sfxinfo_t *sfxinfo, int channel, int vol, int sep)
{
    cachedsound_t *cached;
    voice_t *voice;

    if (!sound_ready || channel < 0 || channel >= NUM_CHANNELS)
    {
        return -1;
    }

    cached = CacheSound(sfxinfo);

    if (cached == NULL)
    {
        return -1;
    }

    voice = &voices[channel];
    voice->samples = cached->samples;
    voice->length = cached->length;
    voice->position = 0;
    voice->step = (unsigned int)
        (((unsigned long long) cached->rate << 16) / output_rate);
    SetVoiceVolume(voice, vol, sep);
    voice->playing = true;

    // DOOM uses the return value as the handle it later asks about, and the
    // channel index is the only thing it needs to be.
    return channel;
}

static void HAL_StopSound(int channel)
{
    if (channel < 0 || channel >= NUM_CHANNELS)
    {
        return;
    }

    voices[channel].playing = false;
}

static boolean HAL_SoundIsPlaying(int channel)
{
    if (channel < 0 || channel >= NUM_CHANNELS)
    {
        return false;
    }

    return voices[channel].playing;
}

// Sounds are loaded the first time they are played, so there is nothing to do
// here; precaching a whole level's effects would only add a pause.
static void HAL_CacheSounds(sfxinfo_t *sounds, int num_sounds)
{
    (void) sounds;
    (void) num_sounds;
}

sound_module_t DG_sound_module =
{
    sound_devices,
    arrlen(sound_devices),
    HAL_InitSound,
    HAL_ShutdownSound,
    HAL_GetSfxLumpNum,
    HAL_UpdateSound,
    HAL_UpdateSoundParams,
    HAL_StartSound,
    HAL_StopSound,
    HAL_SoundIsPlaying,
    HAL_CacheSounds,
};

//
// Music.
//
// Not implemented. DOOM's music is MUS, which would have to be converted to
// MIDI and then synthesised — an OPL emulator or a wavetable, either of which
// is a larger piece of work than the whole sound path above.
//
// The engine registers a music module unconditionally and never checks whether
// Init succeeded, so every entry below has to be a working no-op rather than
// absent. RegisterSong returning NULL is what the rest of the engine expects
// from a song it cannot play, so the game runs with sound effects and silence
// where the music would be.
//

static snddevice_t music_devices[] =
{
    SNDDEVICE_GENMIDI,
};

static boolean HAL_InitMusic(void)
{
    return false;
}

static void HAL_ShutdownMusic(void)
{
}

static void HAL_SetMusicVolume(int volume)
{
    (void) volume;
}

static void HAL_PauseMusic(void)
{
}

static void HAL_ResumeMusic(void)
{
}

static void *HAL_RegisterSong(void *data, int len)
{
    (void) data;
    (void) len;
    return NULL;
}

static void HAL_UnRegisterSong(void *handle)
{
    (void) handle;
}

static void HAL_PlaySong(void *handle, boolean looping)
{
    (void) handle;
    (void) looping;
}

static void HAL_StopSong(void)
{
}

static boolean HAL_MusicIsPlaying(void)
{
    return false;
}

static void HAL_PollMusic(void)
{
}

music_module_t DG_music_module =
{
    music_devices,
    arrlen(music_devices),
    HAL_InitMusic,
    HAL_ShutdownMusic,
    HAL_SetMusicVolume,
    HAL_PauseMusic,
    HAL_ResumeMusic,
    HAL_RegisterSong,
    HAL_UnRegisterSong,
    HAL_PlaySong,
    HAL_StopSong,
    HAL_MusicIsPlaying,
    HAL_PollMusic,
};
