@/Users/junie/.codex/RTK.md

# Parallels 测试约定

- Windows 11 与 Ubuntu 26.04 虚拟机不得同时运行。启动其中一台前，先确认另一台已经关闭；完成当前平台测试后再切换。
- 不使用 Computer Use 点击或输入 Parallels 来宾系统界面，因为鼠标和键盘事件无法可靠传入虚拟机。
- 虚拟机操作优先使用 `prlctl exec` 和 `~/Parallels Shared` 共享目录。
- 如果测试必须经过来宾系统 GUI，停止自动操作并请用户手动完成该步骤，然后从命令行继续验收。
- 两台虚拟机中的开发环境和编译缓存可以保留，便于后续跨平台测试。
