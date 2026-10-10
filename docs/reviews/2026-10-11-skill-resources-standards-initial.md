# Standards 初版：技能参考资源

独立Standards累计基准 `origin/main=4b2357fcf01f47ba08d7724edbba7accfb60972c`；初版固定 `55068dc7a5a07429525cff5d6237b3d28c663af3`，fresh `fa21243a..55068dc7` 九文件。继承历史结论，不声称穷尽重审112提交；用户生产级要求及既有能力合同适用，工具强制规则排除。

初版新增硬性问题0、可行动Fowler启发式0。注册表拥有声明/版本选择，短锁后释放再执行能力I/O；共享实际worker槽、只读NoEffect与原白名单保留。具体SkillResource承载path/hash，预算明确，不引入环境路径或来源认证。已读初版六组实际日志，未自行执行或认证Rust/官方CI。

随后固定 `5a5cc77f9914f78810994a2883ef75b45c132577` 的复查发现一项P2：长路径测试留下两个文件，后续父链接负例的 `references.rmdir()` 失败，完整验收无法结束。实际证据 `/tmp/jiaclaw-oct11-resource-process-final.log`，不能借初版六PASS冒充此head通过。这是验收夹具缺陷，不是生产权限失败。另交付最终日志仍读活跃writer的生命周期提示，未冒充该位置已观察到UTF-8失败。

两固定版本结论分别保留；见 [终审](2026-10-11-skill-resources-standards.md)。
