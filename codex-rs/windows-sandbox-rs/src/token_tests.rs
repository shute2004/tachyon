use super::*;
use std::os::windows::io::AsRawHandle;
use std::os::windows::io::FromRawHandle;
use std::os::windows::io::OwnedHandle;
use windows_sys::Win32::Security::AccessCheck;
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows_sys::Win32::Security::Authorization::SDDL_REVISION_1;
use windows_sys::Win32::Security::DuplicateToken;
use windows_sys::Win32::Security::EqualSid;
use windows_sys::Win32::Security::GENERIC_MAPPING;
use windows_sys::Win32::Security::MapGenericMask;
use windows_sys::Win32::Security::SecurityImpersonation;
use windows_sys::Win32::Security::TokenRestrictedSids;
use windows_sys::Win32::Storage::FileSystem::DELETE;
use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
use windows_sys::Win32::Storage::FileSystem::FILE_DELETE_CHILD;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_EXECUTE;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_READ;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_WRITE;
use windows_sys::Win32::Storage::FileSystem::FILE_READ_DATA;
use windows_sys::Win32::Storage::FileSystem::FILE_WRITE_DATA;

unsafe fn token_has_restricting_sid(token: HANDLE, expected_sid: *mut c_void) -> Result<bool> {
    let mut needed = 0;
    GetTokenInformation(
        token,
        TokenRestrictedSids,
        std::ptr::null_mut(),
        0,
        &mut needed,
    );
    if needed == 0 {
        return Err(anyhow!(
            "GetTokenInformation(TokenRestrictedSids) size query failed: {}",
            GetLastError()
        ));
    }

    let mut buffer = vec![0_u8; needed as usize];
    if GetTokenInformation(
        token,
        TokenRestrictedSids,
        buffer.as_mut_ptr().cast(),
        needed,
        &mut needed,
    ) == 0
    {
        return Err(anyhow!(
            "GetTokenInformation(TokenRestrictedSids) failed: {}",
            GetLastError()
        ));
    }

    let group_count = std::ptr::read_unaligned(buffer.as_ptr().cast::<u32>()) as usize;
    let after_count = buffer.as_ptr().add(std::mem::size_of::<u32>()) as usize;
    let align = std::mem::align_of::<SID_AND_ATTRIBUTES>();
    let entries_addr = (after_count + (align - 1)) & !(align - 1);
    let restricting_sids =
        std::slice::from_raw_parts(entries_addr as *const SID_AND_ATTRIBUTES, group_count);
    Ok(restricting_sids
        .iter()
        .any(|entry| EqualSid(entry.Sid, expected_sid) != 0))
}

#[test]
fn elevated_token_includes_network_proxy_restricting_sid() -> Result<()> {
    let capability_sid = LocalSid::from_string("S-1-5-21-10-20-30-40")?;
    let network_proxy_sid = LocalSid::from_string("S-1-5-21-50-60-70-80")?;
    let base_token = unsafe { get_current_token_for_restriction()? };
    let restricted_token = unsafe {
        create_readonly_token_with_caps_and_user_from(
            base_token,
            &[capability_sid.as_ptr()],
            &[network_proxy_sid.as_ptr()],
        )?
    };

    let has_network_proxy_sid =
        unsafe { token_has_restricting_sid(restricted_token, network_proxy_sid.as_ptr()) };
    unsafe {
        CloseHandle(restricted_token);
        CloseHandle(base_token);
    }

    assert!(has_network_proxy_sid?);
    Ok(())
}

fn own_token(handle: HANDLE) -> OwnedHandle {
    unsafe { OwnedHandle::from_raw_handle(handle as *mut c_void) }
}

fn raw_token_handle(token: &OwnedHandle) -> HANDLE {
    token.as_raw_handle() as HANDLE
}

fn duplicate_for_access_check(token: HANDLE) -> Result<OwnedHandle> {
    let mut duplicate = 0;
    let (ok, error) = unsafe {
        let ok = DuplicateToken(token, SecurityImpersonation, &mut duplicate);
        (ok, if ok == 0 { GetLastError() } else { 0 })
    };
    if ok == 0 {
        return Err(anyhow!(
            "DuplicateToken(SecurityImpersonation) failed: {error}"
        ));
    }
    Ok(own_token(duplicate))
}

struct LocalSecurityDescriptor(*mut c_void);

impl LocalSecurityDescriptor {
    fn from_sddl(sddl: &str) -> Result<Self> {
        let wide_sddl = crate::winutil::to_wide(sddl);
        let mut descriptor = std::ptr::null_mut();
        let (ok, error) = unsafe {
            let ok = ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide_sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            );
            (ok, if ok == 0 { GetLastError() } else { 0 })
        };
        if ok == 0 {
            return Err(anyhow!(
                "ConvertStringSecurityDescriptorToSecurityDescriptorW failed: {error}"
            ));
        }
        Ok(Self(descriptor))
    }
}

impl Drop for LocalSecurityDescriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0 as HLOCAL);
        }
    }
}

fn access_check(token: HANDLE, descriptor: &LocalSecurityDescriptor, right: u32) -> Result<bool> {
    let mapping = GENERIC_MAPPING {
        GenericRead: FILE_GENERIC_READ,
        GenericWrite: FILE_GENERIC_WRITE,
        GenericExecute: FILE_GENERIC_EXECUTE,
        GenericAll: FILE_ALL_ACCESS,
    };
    let mut desired_access = right;
    unsafe { MapGenericMask(&mut desired_access, &mapping) };

    // AccessCheck writes a variable-sized PRIVILEGE_SET; this aligned, bounded buffer is ample
    // for the token's privileges and turns an unexpected size requirement into an API error.
    let mut privilege_set = [0usize; 128];
    let mut privilege_set_length = std::mem::size_of_val(&privilege_set) as u32;
    let mut granted_access = 0;
    let mut access_status = 0;
    let (ok, error) = unsafe {
        let ok = AccessCheck(
            descriptor.0,
            token,
            desired_access,
            &mapping,
            privilege_set.as_mut_ptr().cast(),
            &mut privilege_set_length,
            &mut granted_access,
            &mut access_status,
        );
        (ok, if ok == 0 { GetLastError() } else { 0 })
    };
    if ok == 0 {
        return Err(anyhow!("AccessCheck failed for {right:#x}: {error}"));
    }
    Ok(access_status != 0)
}

fn file_rights(
    token: HANDLE,
    descriptor: &LocalSecurityDescriptor,
) -> Result<(bool, bool, bool, bool)> {
    Ok((
        access_check(token, descriptor, FILE_READ_DATA)?,
        access_check(token, descriptor, FILE_WRITE_DATA)?,
        access_check(token, descriptor, DELETE)?,
        access_check(token, descriptor, FILE_DELETE_CHILD)?,
    ))
}

#[test]
fn write_restricted_access_check_requires_capability_for_delete_rights() -> Result<()> {
    const CAPABILITY_SID: &str = "S-1-5-21-4242424242-4242424242-4242424242-4242";
    let capability_sid = LocalSid::from_string(CAPABILITY_SID)?;
    let base_primary = own_token(unsafe { get_current_token_for_restriction()? });
    let user_sid = unsafe { get_user_sid_bytes(raw_token_handle(&base_primary))? };
    let user_sid = crate::winutil::string_from_sid_bytes(&user_sid).map_err(anyhow::Error::msg)?;
    let restricted_primary = own_token(unsafe {
        create_workspace_write_token_with_caps_from(
            raw_token_handle(&base_primary),
            &[capability_sid.as_ptr()],
        )?
    });
    let base_token = duplicate_for_access_check(raw_token_handle(&base_primary))?;
    let restricted_token = duplicate_for_access_check(raw_token_handle(&restricted_primary))?;

    let no_capability_sddl = format!("O:{user_sid}G:{user_sid}D:P(A;;FA;;;{user_sid})(A;;FR;;;WD)");
    let capability_sddl = format!(
        "O:{user_sid}G:{user_sid}D:P(A;;FA;;;{user_sid})(A;;FR;;;WD)(A;;FA;;;{CAPABILITY_SID})"
    );
    let no_capability_descriptor = LocalSecurityDescriptor::from_sddl(&no_capability_sddl)?;
    let capability_descriptor = LocalSecurityDescriptor::from_sddl(&capability_sddl)?;

    let observations = (
        file_rights(raw_token_handle(&base_token), &no_capability_descriptor)?,
        file_rights(
            raw_token_handle(&restricted_token),
            &no_capability_descriptor,
        )?,
        file_rights(raw_token_handle(&restricted_token), &capability_descriptor)?,
    );
    assert_eq!(
        observations,
        (
            (true, true, true, true),
            (true, false, false, false),
            (true, true, true, true),
        ),
        "rights tuple order is (FILE_READ_DATA, FILE_WRITE_DATA, DELETE, FILE_DELETE_CHILD); token order is (base control, restricted without capability ACE, restricted with capability ACE)"
    );
    Ok(())
}
