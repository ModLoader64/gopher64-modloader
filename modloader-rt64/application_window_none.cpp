#include "hle/rt64_application_window.h"

#include "imgui/imgui_impl_sdl2_custom.h"

namespace RT64 {
    ApplicationWindow *ApplicationWindow::HookedApplicationWindow = nullptr;

    ApplicationWindow::ApplicationWindow() {
        listener = nullptr;
    }

    ApplicationWindow::~ApplicationWindow() { }

    void ApplicationWindow::setup(RenderWindow window, Listener *listener, uint32_t threadId) {
        windowHandle = window;
        this->listener = listener;
    }

    void ApplicationWindow::setup(const char *windowTitle, Listener *listener) {
        this->listener = listener;
    }

    void ApplicationWindow::setFullScreen(bool newFullScreen) {
        fullScreen = newFullScreen;
    }

    void ApplicationWindow::makeResizable() { }

    void ApplicationWindow::detectRefreshRate() { }

    uint32_t ApplicationWindow::getRefreshRate() const {
        return refreshRate;
    }

    bool ApplicationWindow::detectWindowMoved() {
        return false;
    }

    void ApplicationWindow::sdlCheckFilterInstallation() { }

    int ApplicationWindow::sdlEventFilter(void *userdata, SDL_Event *event) {
        return 0;
    }

#ifdef _WIN32
    void ApplicationWindow::windowMessage(UINT message, WPARAM wParam, LPARAM lParam) { }

    LRESULT CALLBACK ApplicationWindow::windowHookCallback(int nCode, WPARAM wParam, LPARAM lParam) {
        return CallNextHookEx(nullptr, nCode, wParam, lParam);
    }
#endif
};

bool ImGui_ImplSDL2_InitForOther(SDL_Window *window) {
    return false;
}

void ImGui_ImplSDL2_Shutdown() { }

void ImGui_ImplSDL2_NewFrame() { }

bool ImGui_ImplSDL2_ProcessEvent(const SDL_Event *event) {
    return false;
}
