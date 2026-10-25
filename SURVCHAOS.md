# GHost Reforged для survchaos.ru

Сборка на основе ветки `feat/mmr` (shideneyu/ghost-reforged) с изменениями:

- сервер рейтинга — `https://survchaos.ru` (другой адрес: переменная окружения `W3LADDER_API`);
- проверенные версии Warcraft III: 3.0.0.24268 и 3.0.1.24371;
- обновления программы берутся из `bylatprovzor/ghost-reforged`, а не у автора.

## Настройка на ПК с ботом (один раз)

1. На сервере: `w3ladder-admin bot add eu-1 eu` — напечатает ключ.
2. На ПК с ботом в PowerShell (подставьте ключ):
   ```
   setx W3LADDER_COLLECTOR_KEY "КЛЮЧ"
   setx W3LADDER_MODE "test"
   ```
   `test` — проверочные игры не попадают в рейтинг. Для настоящих игр: `setx W3LADDER_MODE "ranked"`.
3. Перезапустить бота (переменные читаются при запуске).
4. Карта: `SurvivalChaosRank_v1.59_survchaos.w3x` в `Документы\Warcraft III\Maps`.
5. В «Options»: хост наблюдателем (observer), «leave on load» и «restart on load» — бот выходит после загрузки и открывает новое лобби; результат присылает карта DNS-отчётами.
