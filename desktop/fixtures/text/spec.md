# Lantern: a pocket weather station (draft notes)

Date: 2026-01-12. Status: invented example for tests; not a real product.
Lantern is a small solar device that reads wind, rain and air quality and shows them on a phone.
The notes are shaped like a short specification: headings, a table, a list and code.

## Goal

A hiker can open the Lantern page, see the last hour of wind and rain, and get a short warning
when a storm is likely, without any account and without sending data anywhere else.

## Decisions

| # | Decision |
|---|---|
| D1 | Wind is read every 10 seconds and averaged per minute. |
| D2 | Rain uses a tipping bucket, 0.2 mm per tip. |
| D3 | The case is recycled aluminium, painted matte grey. |
| D4 | Battery life target: 30 days without sun. |
| D5 | Charging by a small solar panel on the top. |
| D6 | Radio: Bluetooth Low Energy only in version 1. |
| D7 | The phone page is a plain web page, no app store. |
| D8 | Data stays on the device for 14 days. |
| D9 | A warning is a sound plus a red band on the page. |
| D10 | Air quality is shown as three colours, not a number. |
| D11 | The firmware is updated by cable, not over the air. |
| D12 | Temperature is shown in Celsius with a switch to Fahrenheit. |
| D13 | The device name is set once, at first start. |
| D14 | No microphone, no camera, no location. |
| D15 | The base price target is 89 euros. |
| D16 | Sold with a wall clip and a 1 m cable. |
| D17 | Waterproof to the IP65 level. |
| D18 | The logo is a small lamp, engraved. |
| D19 | The manual is one page, with pictures. |
| D20 | Spare parts: the bucket and the clip. |
| D21 | Version 2 may add a second device for the valley. |
| D22 | Colour of the status light: green, amber, red. |
| D23 | The page works offline once opened. |
| D24 | Support answers within two working days. |
| D25 | Every unit is tested in a wind tunnel. |

## Hardware

- A small board with a low power processor.
- A wind sensor with no moving parts (ultrasonic).
- A rain bucket with a magnet and a reed switch.
- An air sensor for fine dust.
- A 2000 mAh battery.

## Software

The firmware keeps one record per minute. A record looks like `{ wind: 4.2, rain: 0.0, air: 2 }` and is
stored in a ring of 20,160 entries (14 days). The page asks for the ring in chunks of 60 records.

```
GET /records?from=1736640000&count=60
```

## Warnings

This part of the notes covers warnings for Lantern. The device is small and the text is short:
each point is one sentence, and each decision above has a number that this part refers to.

1. Read the decisions that touch warnings.
2. Write the test before the change.
3. Check the result on a real device.

## Power

This part of the notes covers power for Lantern. The device is small and the text is short:
each point is one sentence, and each decision above has a number that this part refers to.

1. Read the decisions that touch power.
2. Write the test before the change.
3. Check the result on a real device.

## Privacy

This part of the notes covers privacy for Lantern. The device is small and the text is short:
each point is one sentence, and each decision above has a number that this part refers to.

1. Read the decisions that touch privacy.
2. Write the test before the change.
3. Check the result on a real device.

## Testing

This part of the notes covers testing for Lantern. The device is small and the text is short:
each point is one sentence, and each decision above has a number that this part refers to.

1. Read the decisions that touch testing.
2. Write the test before the change.
3. Check the result on a real device.

## Packaging

This part of the notes covers packaging for Lantern. The device is small and the text is short:
each point is one sentence, and each decision above has a number that this part refers to.

1. Read the decisions that touch packaging.
2. Write the test before the change.
3. Check the result on a real device.

## Schedule

This part of the notes covers schedule for Lantern. The device is small and the text is short:
each point is one sentence, and each decision above has a number that this part refers to.

1. Read the decisions that touch schedule.
2. Write the test before the change.
3. Check the result on a real device.

## Costs

This part of the notes covers costs for Lantern. The device is small and the text is short:
each point is one sentence, and each decision above has a number that this part refers to.

1. Read the decisions that touch costs.
2. Write the test before the change.
3. Check the result on a real device.

## Risks

This part of the notes covers risks for Lantern. The device is small and the text is short:
each point is one sentence, and each decision above has a number that this part refers to.

1. Read the decisions that touch risks.
2. Write the test before the change.
3. Check the result on a real device.

## Support

This part of the notes covers support for Lantern. The device is small and the text is short:
each point is one sentence, and each decision above has a number that this part refers to.

1. Read the decisions that touch support.
2. Write the test before the change.
3. Check the result on a real device.

## Names

This part of the notes covers names for Lantern. The device is small and the text is short:
each point is one sentence, and each decision above has a number that this part refers to.

1. Read the decisions that touch names.
2. Write the test before the change.
3. Check the result on a real device.

## Open points

- Whether the warning sound can be turned off.
- Whether the valley device needs its own page.
- The final colour of the case.
