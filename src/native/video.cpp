#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <mfapi.h>
#include <mfidl.h>
#include <mfreadwrite.h>
#include <propvarutil.h>
#include <wrl/client.h>
#include <new>
#include <memory>
#include <cstdint>

using Microsoft::WRL::ComPtr;

// Keep optional codec DLLs out of the launcher's static import table.
// Windows editions without Media Foundation can still show the static artwork.
struct Platform {
    HMODULE core = LoadLibraryExW(L"mfplat.dll", nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32);
    HMODULE source = LoadLibraryExW(L"mfreadwrite.dll", nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32);
    decltype(&MFStartup) startup = core ? reinterpret_cast<decltype(&MFStartup)>(GetProcAddress(core, "MFStartup")) : nullptr;
    decltype(&MFShutdown) shutdown = core ? reinterpret_cast<decltype(&MFShutdown)>(GetProcAddress(core, "MFShutdown")) : nullptr;
    decltype(&MFCreateAttributes) attributes = core ? reinterpret_cast<decltype(&MFCreateAttributes)>(GetProcAddress(core, "MFCreateAttributes")) : nullptr;
    decltype(&MFCreateMediaType) type = core ? reinterpret_cast<decltype(&MFCreateMediaType)>(GetProcAddress(core, "MFCreateMediaType")) : nullptr;
    decltype(&MFGetStrideForBitmapInfoHeader) stride = core ? reinterpret_cast<decltype(&MFGetStrideForBitmapInfoHeader)>(GetProcAddress(core, "MFGetStrideForBitmapInfoHeader")) : nullptr;
    decltype(&MFCreateSourceReaderFromURL) reader = source ? reinterpret_cast<decltype(&MFCreateSourceReaderFromURL)>(GetProcAddress(source, "MFCreateSourceReaderFromURL")) : nullptr;
    bool available() const { return startup && shutdown && attributes && type && stride && reader; }
};

static Platform& platform() { static Platform value; return value; }

struct Video {
    ComPtr<IMFSourceReader> reader;
    UINT32 width = 0;
    UINT32 height = 0;
    bool com = false;
    bool media = false;
    ~Video() {
        reader.Reset();
        if (media) platform().shutdown();
        if (com) CoUninitialize();
    }
};

extern "C" void ghost_video_close(Video* video) { delete video; }

extern "C" Video* ghost_video_open(const wchar_t* path, UINT32* width, UINT32* height) {
    if (!platform().available()) return nullptr;
    std::unique_ptr<Video> video(new (std::nothrow) Video());
    if (!video) return nullptr;
    HRESULT result = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    video->com = SUCCEEDED(result);
    if (!video->com) return nullptr;
    result = platform().startup(MF_VERSION, MFSTARTUP_LITE);
    video->media = SUCCEEDED(result);
    if (!video->media) return nullptr;
    ComPtr<IMFAttributes> attributes;
    ComPtr<IMFMediaType> output;
    ComPtr<IMFMediaType> actual;
    if (FAILED(platform().attributes(&attributes, 2)) ||
        FAILED(attributes->SetUINT32(MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, TRUE)) ||
        FAILED(platform().reader(path, attributes.Get(), &video->reader)) ||
        FAILED(video->reader->SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS, FALSE)) ||
        FAILED(video->reader->SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM, TRUE)) ||
        FAILED(platform().type(&output)) ||
        FAILED(output->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video)) ||
        FAILED(output->SetGUID(MF_MT_SUBTYPE, MFVideoFormat_RGB32)) ||
        FAILED(video->reader->SetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM, nullptr, output.Get())) ||
        FAILED(video->reader->GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM, &actual)) ||
        FAILED(MFGetAttributeSize(actual.Get(), MF_MT_FRAME_SIZE, &video->width, &video->height)) ||
        video->width == 0 || video->height == 0 || video->width > 1920 || video->height > 1080) {
        return nullptr;
    }
    *width = video->width;
    *height = video->height;
    return video.release();
}

static void copy_rows(const BYTE* source, LONG stride, UINT32 width, UINT32 height, uint8_t* rgba) {
    for (UINT32 y = 0; y < height; ++y) {
        const BYTE* row = source + static_cast<ptrdiff_t>(y) * stride;
        uint8_t* target = rgba + static_cast<size_t>(y) * width * 4;
        for (UINT32 x = 0; x < width; ++x) {
            target[x*4] = row[x*4+2];
            target[x*4+1] = row[x*4+1];
            target[x*4+2] = row[x*4];
            target[x*4+3] = 255;
        }
    }
}

extern "C" bool ghost_video_convert_buffer(const uint8_t* bytes, size_t length, LONG stride,
    UINT32 width, UINT32 height, uint8_t* rgba, size_t capacity) {
    if (!bytes || !rgba || width == 0 || height == 0 || width > 1920 || height > 1080 ||
        capacity != static_cast<size_t>(width) * height * 4) return false;
    const uint64_t pitch = stride < 0 ? -static_cast<int64_t>(stride) : stride;
    if (pitch < width * 4 || pitch * height > length) return false;
    const BYTE* first = stride < 0 ? bytes + pitch * (height-1) : bytes;
    copy_rows(first, stride, width, height, rgba);
    return true;
}

extern "C" int ghost_video_read(Video* video, uint8_t* rgba, size_t capacity) {
    if (!video || capacity != static_cast<size_t>(video->width) * video->height * 4) return -1;
    for (int attempt = 0; attempt < 32; ++attempt) {
        ComPtr<IMFSample> sample;
        DWORD flags = 0;
        if (FAILED(video->reader->ReadSample(MF_SOURCE_READER_FIRST_VIDEO_STREAM, 0, nullptr, &flags, nullptr, &sample))) return -2;
        if (flags & MF_SOURCE_READERF_ENDOFSTREAM) {
            PROPVARIANT zero;
            InitPropVariantFromInt64(0, &zero);
            HRESULT result = video->reader->SetCurrentPosition(GUID_NULL, zero);
            PropVariantClear(&zero);
            if (FAILED(result)) return -3;
            continue;
        }
        if (flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED) {
            ComPtr<IMFMediaType> actual;
            UINT32 width = 0, height = 0;
            GUID subtype = GUID_NULL;
            if (FAILED(video->reader->GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM, &actual)) ||
                FAILED(MFGetAttributeSize(actual.Get(), MF_MT_FRAME_SIZE, &width, &height)) ||
                FAILED(actual->GetGUID(MF_MT_SUBTYPE, &subtype)) || subtype != MFVideoFormat_RGB32 ||
                width != video->width || height != video->height) return -4;
        }
        if (!sample) continue;
        ComPtr<IMFMediaBuffer> buffer;
        if (FAILED(sample->ConvertToContiguousBuffer(&buffer))) return -5;
        BYTE* bytes = nullptr;
        DWORD length = 0;
        ComPtr<IMF2DBuffer> plane;
        LONG stride = static_cast<LONG>(video->width * 4);
        bool two_dimensional = SUCCEEDED(buffer.As(&plane));
        if (two_dimensional) {
            if (FAILED(plane->Lock2D(&bytes, &stride))) return -6;
        } else {
            ComPtr<IMFMediaType> actual;
            UINT32 encoded_stride = 0;
            if (FAILED(video->reader->GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM, &actual))) return -10;
            if (SUCCEEDED(actual->GetUINT32(MF_MT_DEFAULT_STRIDE, &encoded_stride))) {
                stride = static_cast<LONG>(encoded_stride);
            } else if (FAILED(platform().stride(MFVideoFormat_RGB32.Data1, video->width, &stride))) {
                return -10;
            }
            if (FAILED(buffer->Lock(&bytes, nullptr, &length))) return -7;
            bool converted = ghost_video_convert_buffer(bytes, length, stride, video->width, video->height, rgba, capacity);
            buffer->Unlock();
            return converted ? 1 : -8;
        }
        copy_rows(bytes, stride, video->width, video->height, rgba);
        plane->Unlock2D();
        return 1;
    }
    return -9;
}
