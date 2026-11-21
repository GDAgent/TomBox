// Desktop GDK user startup for the package probe.
#pragma once

#include <XAsync.h>
#include <XGameErr.h>
#include <XUser.h>
#include <atomic>
#include <cstdio>

inline bool check_silent_user(XTaskQueueHandle queue) {
    bool completed = false;
    XAsyncBlock async{};
    async.queue = queue;
    async.context = &completed;
    async.callback = [](XAsyncBlock* block) { *static_cast<bool*>(block->context) = true; };
    HRESULT hr = XUserAddAsync(XUserAddOptions::AddDefaultUserSilently, &async);
    if (FAILED(hr)) {
        std::fprintf(stderr, "XUserAddAsync returned %#lx\n", static_cast<unsigned long>(hr));
        return false;
    }
    // Only this thread dispatches completions. Keep the async block alive until its callback runs.
    const auto start = GetTickCount64();
    while (!completed) {
        XTaskQueueDispatch(queue, XTaskQueuePort::Completion, 100);
        if (!completed && GetTickCount64() - start >= 15000) {
            std::fputs("Silent user add timed out after 15 seconds\n", stderr);
            std::fflush(stderr);
            ExitProcess(1);
        }
    }
    XUserHandle user = nullptr;
    hr = XUserAddResult(&async, &user);
    if (hr == E_GAMEUSER_NO_DEFAULT_USER && !user) {
        std::puts("Silent user add completed with E_GAMEUSER_NO_DEFAULT_USER.");
        return true;
    }
    if (FAILED(hr) || !user) {
        std::fprintf(stderr, "XUserAddResult returned %#lx (handle %s)\n",
                     static_cast<unsigned long>(hr), user ? "present" : "absent");
        return false;
    }
    XUserLocalId id{};
    XUserHandle found = nullptr;
    XUserState state{};
    bool ok = SUCCEEDED(XUserGetState(user, &state)) && state == XUserState::SignedIn
        && SUCCEEDED(XUserGetLocalId(user, &id)) && SUCCEEDED(XUserFindUserByLocalId(id, &found))
        && found && XUserCompare(user, found) == 0;
    if (found) XUserCloseHandle(found);
    XUserCloseHandle(user);
    std::puts(ok ? "Signed-in user matched its local ID lookup." : "Signed-in user lookup failed.");
    return ok;
}

inline bool check_package_users() {
    uint32_t max_users = 0;
    HRESULT hr = XUserGetMaxUsers(&max_users);
    std::printf("XUserGetMaxUsers returned %#lx, maximum %u.\n", static_cast<unsigned long>(hr), max_users);
    XTaskQueueHandle queue = nullptr;
    const HRESULT queue_hr = XTaskQueueCreate(XTaskQueueDispatchMode::ThreadPool, XTaskQueueDispatchMode::Manual, &queue);
    if (FAILED(queue_hr)) {
        std::fprintf(stderr, "XTaskQueueCreate returned %#lx\n", static_cast<unsigned long>(queue_hr));
        return false;
    }
    std::atomic<unsigned> changes{0};
    XTaskQueueRegistrationToken token{};
    // Deliver change events on the runtime queue so unregister can wait for callbacks.
    const HRESULT reg = XUserRegisterForChangeEvent(nullptr, &changes,
        [](void* ctx, XUserLocalId, XUserChangeEvent) { ++*static_cast<std::atomic<unsigned>*>(ctx); }, &token);
    std::printf("XUserRegisterForChangeEvent returned %#lx.\n", static_cast<unsigned long>(reg));
    const bool added = check_silent_user(queue);
    bool removed = false;
    if (SUCCEEDED(reg)) {
        removed = XUserUnregisterForChangeEvent(token, true);
        std::printf("Unregistered: %s. User change notifications observed: %u.\n",
                    removed ? "yes" : "no", changes.load());
    }
    XTaskQueueCloseHandle(queue);
    return SUCCEEDED(hr) && max_users && added && removed;
}
