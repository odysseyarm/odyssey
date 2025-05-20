use windows::Win32::{
    Foundation::{FALSE, GENERIC_ALL},
    Security::{
        Authorization::{
            SetEntriesInAclW, EXPLICIT_ACCESS_W, NO_MULTIPLE_TRUSTEE, SET_ACCESS, TRUSTEE_IS_GROUP, TRUSTEE_IS_SID, TRUSTEE_W
        }, CreateWellKnownSid, GetSecurityDescriptorDacl, InitializeSecurityDescriptor, SetSecurityDescriptorDacl, WinWorldSid, ACL, DACL_SECURITY_INFORMATION, NO_INHERITANCE, PSECURITY_DESCRIPTOR, PSID, SECURITY_DESCRIPTOR, SUB_CONTAINERS_AND_OBJECTS_INHERIT
    },
    System::{
        Services::{
            QueryServiceObjectSecurity, SetServiceObjectSecurity,
            SC_HANDLE,
        },
        SystemServices::SECURITY_DESCRIPTOR_REVISION,
    },
};

use windows_core::PWSTR;

#[cfg(windows)]
fn main() -> windows_service::Result<()> {
    use std::ffi::OsString;
    use windows_service::{
        service::{ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceType},
        service_manager::{ServiceManager, ServiceManagerAccess},
    };

    let manager_access = ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE;
    let service_manager = ServiceManager::local_computer(None::<&str>, manager_access)?;

    let service_binary_path = ::std::env::current_exe()
        .unwrap()
        .with_file_name("service.exe");

    let service_info = ServiceInfo {
        name: OsString::from("OdysseyService"),
        display_name: OsString::from("Odyssey Service"),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::OnDemand,
        error_control: ServiceErrorControl::Normal,
        executable_path: service_binary_path,
        launch_arguments: vec![],
        dependencies: vec![],
        // account_name: Some("NT AUTHORITY\\LocalService".into()),
        account_name: None,
        account_password: None,
    };

    let service = service_manager.create_service(&service_info, ServiceAccess::ALL_ACCESS)?;
    service.set_description("Exposes ATS device interfaces to the Odyssey Desktop app and other apps that use the Odyssey client library")?;

    println!("Updating service security");
    unsafe {
        match update_service_security(windows::Win32::System::Services::SC_HANDLE(service.raw_handle())) {
            Ok(_) => println!("Service security updated successfully."),
            Err(e) => eprintln!("Failed to update service security: {}", e),
        }
    }

    Ok(())
}

#[cfg(not(windows))]
fn main() {
    panic!("This program is only intended to run on Windows.");
}

unsafe fn update_service_security(sc_handle: SC_HANDLE) -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes_needed = 0;
    let mut security_descriptor = SECURITY_DESCRIPTOR::default();
    let psecurity_descriptor = PSECURITY_DESCRIPTOR(&mut security_descriptor as *mut _ as _);

    println!("Querying service object security");

    // first call to get the needed buffer size
    let _ = QueryServiceObjectSecurity(
        sc_handle,
        DACL_SECURITY_INFORMATION.0,
        Some(psecurity_descriptor),
        0,
        &mut bytes_needed,
    );

    println!("Querying service object security, bytes_needed: {}", bytes_needed);

    QueryServiceObjectSecurity(
        sc_handle,
        DACL_SECURITY_INFORMATION.0,
        Some(psecurity_descriptor),
        bytes_needed,
        &mut bytes_needed,
    )?;

    let mut acl_ptr = std::ptr::null_mut::<ACL>();
    let mut dacl_present = FALSE;
    let mut dacl_default = FALSE;

    println!("Getting DACL from service object security");

    GetSecurityDescriptorDacl(
        psecurity_descriptor,
        &mut dacl_present,
        &mut acl_ptr,
        &mut dacl_default,
    )?;

    let mut everyone_psid = [0u8; windows::Win32::Storage::FileSystem::MAX_SID_SIZE as usize];
    let mut everyone_psid_len = u32::try_from(everyone_psid.len()).unwrap();
    CreateWellKnownSid(
        WinWorldSid,
        None,
        Some(PSID(everyone_psid.as_mut_ptr() as _)),
        &mut everyone_psid_len,
    )?;

    let explitic_access = [EXPLICIT_ACCESS_W {
        grfAccessPermissions: GENERIC_ALL.0,
        grfAccessMode: SET_ACCESS,
        grfInheritance: NO_INHERITANCE | SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Trustee: TRUSTEE_W {
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_GROUP,
            pMultipleTrustee: std::ptr::null_mut(),
            ptstrName: PWSTR(everyone_psid.as_mut_ptr() as *mut _),
            ..Default::default()
        },
    }];

    // println!("{}", PWSTR(everyone_psid.0.cast()).to_string()?);

    let mut new_acl_ptr = std::ptr::null_mut::<ACL>();
    println!("Setting entries in ACL");
    SetEntriesInAclW(
        Some(&explitic_access),
        None,
        &mut new_acl_ptr,
    )
    .ok()?;
    println!("Creating new ACL");
    let new_acl = new_acl_ptr.as_ref().ok_or("new_acl_ptr is null")?;

    println!("Initializing new security descriptor");
    // Initialize a new security descriptor.
    let mut security_desc = SECURITY_DESCRIPTOR::default();
    let new_security_descriptor =
        PSECURITY_DESCRIPTOR((&mut security_desc as *mut SECURITY_DESCRIPTOR).cast());

    InitializeSecurityDescriptor(new_security_descriptor, SECURITY_DESCRIPTOR_REVISION)?;
    // let new_security_descriptor = new_security_descriptor.0 as *mut SECURITY_DESCRIPTOR;

    println!("Setting DACL in new security descriptor");
    // Set the new DACL in the security descriptor.
    SetSecurityDescriptorDacl(new_security_descriptor, true, Some(new_acl), false)?;

    println!("Setting service object security");
    // Set the new DACL for the service object.
    SetServiceObjectSecurity(
        sc_handle,
        DACL_SECURITY_INFORMATION,
        new_security_descriptor,
    )?;
    Ok(())
}
