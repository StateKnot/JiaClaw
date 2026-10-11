# Standards — wiring

硬规范违反：**0**；Fowler 启发式发现：**0**。

固定 head：`7a092a3eafd0228adf8639572edc1f0b3afa932d`。累计命令 `git diff 4b2357fcf01f47ba08d7724edbba7accfb60972c...7a092a3eafd0228adf8639572edc1f0b3afa932d`；ref解析成功，merge-base等于4b2357fc，累计diff非空，共169项commit。继承b7969dc源码Standards报告及累计历史，不声称穷尽重审；fresh仅 `b7969dcee8e91130da8e244af195691db6df201d..7a092a3eafd0228adf8639572edc1f0b3afa932d` 的 `.github/workflows/release.yml` 一行。

`.github/workflows/release.yml:30` 将实际被安装/更新fixture导入的 `tests/stopped_child.py` 加入PR路径过滤，符合 `docs/skill-sync-validation.md:19` 要求固定head独立四候选资格的规范。未来仅helper变更的PR也会选择该release工作流；原矩阵仍包含Linux/macOS四个平台（`:64–72`），原安装/更新验收命令（`:112–113`）、归档与安装（`:127–136`）均保留。fresh diff只有新增路径；40分钟默认与Intel60分钟预算（`:70/77`）及其他命令未改变。静态接线结论不表示新head官方作业已经通过。

用户生产可用、期限/安全断言、缓存规范及原仓库标准继续适用。没有编译/运行测试、修改仓库或读取Spec报告；因此没有新增缓存或清理行为。

12项均已考虑：Mysterious Name、Duplicated Code、Feature Envy、Data Clumps、Primitive Obsession、Repeated Switches、Shotgun Surgery、Divergent Change、Speculative Generality、Message Chains、Middle Man、Refused Bequest。该路径直接表达已存在的测试依赖，没有新增逻辑或抽象；未发现值得修改的问题。仓库规范优先，工具已覆盖项跳过。
