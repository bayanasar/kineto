#include <stdlib.h>
#include <unistd.h>

#include "my_application.h"

// Kineto is drawn on the GPU everywhere. Under WSLg, Mesa hands a Wayland
// app its software renderer unless the app asks for the GPU Windows passes
// through (/dev/dxg, driven by d3d12), and every frame then takes a tenth
// of a second. A director's own choice of driver is left alone.
static void PreferGpu() {
  if (getenv("GALLIUM_DRIVER") != nullptr) return;
  if (access("/dev/dxg", F_OK) == 0 && access("/usr/lib/wsl/lib/libd3d12.so", F_OK) == 0) {
    setenv("GALLIUM_DRIVER", "d3d12", 0);
  }
}

int main(int argc, char** argv) {
  PreferGpu();
  g_autoptr(MyApplication) app = my_application_new();
  return g_application_run(G_APPLICATION(app), argc, argv);
}
