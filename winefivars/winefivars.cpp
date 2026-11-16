// winefivars.cpp : Enumerate UEFI variables via NtEnumerateSystemEnvironmentValuesEx.
//

#include <windows.h>
#include <winternl.h>
#include <iostream>
#include <iomanip>
#include <vector>
#include <string>

#ifndef NT_SUCCESS
#define NT_SUCCESS(Status) (((NTSTATUS)(Status)) >= 0)
#endif

constexpr NTSTATUS STATUS_BUFFER_TOO_SMALL = static_cast<NTSTATUS>(0xC0000023);
constexpr NTSTATUS STATUS_INFO_LENGTH_MISMATCH = static_cast<NTSTATUS>(0xC0000004);

bool EnableSystemEnvironmentPrivilege()
{
    HANDLE token = nullptr;
    if (!OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &token))
    {
        return false;
    }

    LUID luid{};
    if (!LookupPrivilegeValueW(nullptr, SE_SYSTEM_ENVIRONMENT_NAME, &luid))
    {
        CloseHandle(token);
        return false;
    }

    TOKEN_PRIVILEGES privileges{};
    privileges.PrivilegeCount = 1;
    privileges.Privileges[0].Luid = luid;
    privileges.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;

    SetLastError(ERROR_SUCCESS);
    const bool adjusted = AdjustTokenPrivileges(token, FALSE, &privileges, sizeof(privileges), nullptr, nullptr);
    const DWORD error = GetLastError();
    CloseHandle(token);

    return adjusted && error == ERROR_SUCCESS;
}

namespace
{
    using NtEnumerateSystemEnvironmentValuesExFn = NTSTATUS(NTAPI*)(ULONG, PVOID, PULONG);

    // Information classes accepted by NtEnumerateSystemEnvironmentValuesEx.
    constexpr ULONG VariableInformationNames = 1;   // yields VARIABLE_NAME records
    constexpr ULONG VariableInformationValues = 2;  // yields VARIABLE_NAME_AND_VALUE records

    struct VARIABLE_NAME_AND_VALUE
    {
        ULONG NextEntryOffset;
        ULONG ValueOffset;
        ULONG ValueLength;
        ULONG Attributes;
        GUID  VendorGuid;
        WCHAR Name[1];
    };

    std::wstring FormatGuid(const GUID& guid)
    {
        wchar_t text[40]{};
        StringFromGUID2(guid, text, static_cast<int>(std::size(text)));
        return text;
    }

    void PrintValue(const std::byte* data, ULONG length)
    {
        // UEFI variable data is arbitrary binary, so dump it as hex with a
        // best-effort ASCII gutter rather than pretending it is a string.
        constexpr ULONG perLine = 16;
        for (ULONG offset = 0; offset < length; offset += perLine)
        {
            std::wcout << L"    " << std::hex << std::setfill(L'0') << std::setw(4) << offset << L"  ";

            for (ULONG i = 0; i < perLine; ++i)
            {
                if (offset + i < length)
                {
                    std::wcout << std::setw(2)
                        << static_cast<unsigned>(std::to_integer<unsigned char>(data[offset + i])) << L' ';
                }
                else
                {
                    std::wcout << L"   ";
                }
            }

            std::wcout << L' ';
            for (ULONG i = 0; i < perLine && offset + i < length; ++i)
            {
                const auto byte = std::to_integer<unsigned char>(data[offset + i]);
                std::wcout << (byte >= 0x20 && byte < 0x7f ? static_cast<wchar_t>(byte) : L'.');
            }

            std::wcout << std::dec << std::setfill(L' ') << std::endl;
        }
    }
}

int main()
{
    if (!EnableSystemEnvironmentPrivilege())
    {
        std::wcerr << L"SeSystemEnvironmentPrivilege is unavailable or could not be enabled: "
            << GetLastError() << std::endl;
        return 1;
    }

    HMODULE ntdll = GetModuleHandleW(L"ntdll.dll");
    if (!ntdll)
    {
        std::wcerr << L"Failed to load ntdll.dll" << std::endl;
        return 1;
    }

    auto ntEnumerateSystemEnvironmentValuesEx =
        reinterpret_cast<NtEnumerateSystemEnvironmentValuesExFn>(
            GetProcAddress(ntdll, "NtEnumerateSystemEnvironmentValuesEx"));

    if (!ntEnumerateSystemEnvironmentValuesEx)
    {
        std::wcerr << L"NtEnumerateSystemEnvironmentValuesEx was not found in ntdll.dll" << std::endl;
        return 1;
    }

    ULONG bufferLength = 0;
    NTSTATUS status = ntEnumerateSystemEnvironmentValuesEx(
        VariableInformationValues,
        nullptr,
        &bufferLength);

    if (status != STATUS_BUFFER_TOO_SMALL && status != STATUS_INFO_LENGTH_MISMATCH)
    {
        std::wcerr << L"NtEnumerateSystemEnvironmentValuesEx sizing call failed: 0x"
            << std::hex << static_cast<unsigned long>(status) << std::dec << std::endl;
        return 1;
    }

    std::vector<std::byte> buffer(bufferLength);
    status = ntEnumerateSystemEnvironmentValuesEx(
        VariableInformationValues,
        buffer.data(),
        &bufferLength);

    if (!NT_SUCCESS(status))
    {
        std::wcerr << L"NtEnumerateSystemEnvironmentValuesEx final call failed: 0x"
            << std::hex << static_cast<unsigned long>(status) << std::dec << std::endl;
        return 1;
    }

    const auto* cursor = buffer.data();
    const auto* end = cursor + bufferLength;
    unsigned count = 0;

    while (cursor < end)
    {
        const auto* entry = reinterpret_cast<const VARIABLE_NAME_AND_VALUE*>(cursor);
        const auto remaining = static_cast<ULONG>(end - cursor);

        if (remaining < offsetof(VARIABLE_NAME_AND_VALUE, Name) ||
            entry->ValueOffset < offsetof(VARIABLE_NAME_AND_VALUE, Name) ||
            entry->ValueOffset > remaining ||
            entry->ValueLength > remaining - entry->ValueOffset)
        {
            std::wcerr << L"NtEnumerateSystemEnvironmentValuesEx returned an invalid record" << std::endl;
            return 1;
        }

        // The name is NUL-terminated and padded out to ValueOffset; trim it.
        const auto nameChars =
            (entry->ValueOffset - offsetof(VARIABLE_NAME_AND_VALUE, Name)) / sizeof(WCHAR);
        std::wstring name(entry->Name, nameChars);
        if (const auto nul = name.find(L'\0'); nul != std::wstring::npos)
        {
            name.resize(nul);
        }

        std::wcout << FormatGuid(entry->VendorGuid) << L' ' << name
            << L"  (attributes 0x" << std::hex << entry->Attributes << std::dec
            << L", " << entry->ValueLength << L" bytes)" << std::endl;
        PrintValue(cursor + entry->ValueOffset, entry->ValueLength);
        ++count;

        if (entry->NextEntryOffset == 0)
        {
            break;
        }

        if (entry->NextEntryOffset > remaining)
        {
            std::wcerr << L"NtEnumerateSystemEnvironmentValuesEx returned an invalid entry offset" << std::endl;
            return 1;
        }

        cursor += entry->NextEntryOffset;
    }

    std::wcout << count << L" variable(s)." << std::endl;
    return 0;
}
