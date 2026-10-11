#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <shobjidl.h>
#include <wrl/client.h>
#include <cwchar>

using Microsoft::WRL::ComPtr;

namespace {
struct Apartment {
    HRESULT result = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
    ~Apartment() { if (SUCCEEDED(result)) CoUninitialize(); }
};

HRESULT configure(const wchar_t* path, ComPtr<IFileOpenDialog>& dialog,
                  ComPtr<IShellItem>& root) {
    HRESULT result = SHCreateItemFromParsingName(path, nullptr, IID_PPV_ARGS(root.GetAddressOf()));
    if (FAILED(result)) return result;
    result = CoCreateInstance(__uuidof(FileOpenDialog), nullptr, CLSCTX_INPROC_SERVER,
                              IID_PPV_ARGS(dialog.GetAddressOf()));
    if (FAILED(result)) return result;
    ComPtr<IFileDialog2> restricted;
    result = dialog.As(&restricted);
    if (FAILED(result)) return result;
    // This replaces the navigation pane and prevents browsing above Maps.
    // Failure must never fall back to an unrestricted file picker.
    result = restricted->SetNavigationRoot(root.Get());
    if (FAILED(result)) return result;
    result = dialog->SetFolder(root.Get());
    if (FAILED(result)) return result;
    result = dialog->SetOptions(FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST |
                                FOS_PATHMUSTEXIST | FOS_NOCHANGEDIR | FOS_DONTADDTORECENT);
    if (FAILED(result)) return result;
    const COMDLG_FILTERSPEC filter[] = {{L"Warcraft III maps", L"*.w3x;*.w3m"}};
    result = dialog->SetFileTypes(1, filter);
    if (FAILED(result)) return result;
    return dialog->SetTitle(L"Warcraft III - Maps");
}
}

extern "C" HRESULT ghost_pick_map(const wchar_t* path, wchar_t* output, unsigned capacity) {
    if (!path || !output || capacity == 0) return E_INVALIDARG;
    output[0] = L'\0';
    Apartment apartment;
    if (FAILED(apartment.result)) return apartment.result;
    ComPtr<IFileOpenDialog> dialog;
    ComPtr<IShellItem> root;
    HRESULT result = configure(path, dialog, root);
    if (FAILED(result)) return result;
    result = dialog->Show(nullptr);
    if (result == HRESULT_FROM_WIN32(ERROR_CANCELLED)) return S_FALSE;
    if (FAILED(result)) return result;
    ComPtr<IShellItem> selected;
    result = dialog->GetResult(selected.GetAddressOf());
    if (FAILED(result)) return result;
    wchar_t* name = nullptr;
    result = selected->GetDisplayName(SIGDN_FILESYSPATH, &name);
    if (FAILED(result)) return result;
    const size_t length = std::wcslen(name);
    if (length >= capacity) {
        CoTaskMemFree(name);
        return HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER);
    }
    std::wmemcpy(output, name, length + 1);
    CoTaskMemFree(name);
    return S_OK;
}

extern "C" HRESULT ghost_map_dialog_probe(const wchar_t* path) {
    Apartment apartment;
    if (FAILED(apartment.result)) return apartment.result;
    ComPtr<IFileOpenDialog> dialog;
    ComPtr<IShellItem> root;
    HRESULT result = configure(path, dialog, root);
    if (FAILED(result)) return result;
    ComPtr<IShellItem> folder;
    result = dialog->GetFolder(folder.GetAddressOf());
    if (FAILED(result)) return result;
    int order = 1;
    result = root->Compare(folder.Get(), SICHINT_CANONICAL, &order);
    return FAILED(result) ? result : (order == 0 ? S_OK : E_FAIL);
}
