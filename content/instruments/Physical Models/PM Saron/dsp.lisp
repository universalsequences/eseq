;; PM Saron: a struck bar with passive, frequency-selective radiation build-up.
;; All seven pelog bars and five strike strengths share one modal system.
;; Reproduce its calibration and comparisons: tools/pm-saron/README.md.
;; Source recordings: Latent Sonorities / memeshift, CC BY-NC 4.0.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

(param hardness @group mallet @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)
(param contact @group mallet @default 1 @min 0.3 @max 3 @mod true @mod-mode additive)
(param spread @group mallet @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param dynamics @group mallet @default 1 @min 0 @max 1)
(param decay @group bar @default 1 @min 0.15 @max 4 @mod true @mod-mode additive)
(param loss @group bar @default 1 @min 0.25 @max 4 @mod true @mod-mode additive)
(param inharmonicity @group bar @default 1 @min 0 @max 1.8 @mod true @mod-mode additive)
(param bloom @group bar @default 1 @min 0 @max 2 @mod true @mod-mode additive)
(param color @group bar @default 0 @min -1 @max 1 @mod true @mod-mode additive)
(param register @group voicing @default 0 @min -24 @max 24 @unit st @mod true @mod-mode additive)
(param amount @group tuning @default 1 @min 0 @max 1 @mod true @mod-mode additive)
(param tune @group tuning @default 0 @min -100 @max 100 @unit cents @mod true @mod-mode additive)
(param touch @group damper @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param release_s @group damper @default 0.4 @min 0.02 @max 12 @unit s @mod true @mod-mode additive)
(param lift @group damper @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param width @group output @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param drive @group output @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param tone_hz @group output @default 20000 @min 1000 @max 20000 @unit Hz @mod true @mod-mode additive)
(param gain @group output @default 1 @min 0 @max 4 @mod true @mod-mode additive)

;; BEGIN GENERATED CALIBRATION
;; Latent Sonorities / memeshift: source CC BY-NC 4.0; see ATTRIBUTION.md.
;; Seven bars, five ordinal strike strengths, one 16-mode passive system.
;; No PCM, recorded phases, or per-frame spectral data.
;; Analysis SHA256: d73025e4e3ab3557c3ea3048ead8b832c66814d603712d1925a32836f6ea13e3
(def mode_count 16)
(def ratio_table (tensor @shape [7 16] @data [
  1 5.5469655 5.0534011 2.89935001 4.33744465 8.80089981 8.90457274 2.20579045 12.5390765 2.00098223 16.2173649 9.00693284 5.33862634 7.5829768 3.0014332 13.236264
  1 5.39749915 5.0194672 2.76737717 4.24693193 7.09744143 8.53250634 2.13693405 12.2150398 2.00173047 12.3648237 8.6214295 6.49231476 8.28400092 3.00192446 13.236264
  1 5.5435976 9.1754994 2.96883826 3.87146884 5.39398305 16.2655363 2.06807766 16.7233578 3.36213241 8.51228258 8.23592616 7.64600318 8.98502504 2.3301875 13.236264
  1 5.02430246 9.07563168 2.6921463 3.65905685 3.69052467 12.4182228 1.99922127 14.4760663 4.72253434 4.65974144 7.85042283 8.7996916 9.68604916 2.32900938 13.236264
  1 5.24464477 8.13431143 2.80257837 3.45759909 2.99838684 8.94525669 1.99950368 10.3505895 4.70070344 3.80202372 6.15582781 8.39200792 11.4621736 2.17084197 13.236264
  1 5.42370702 8.07878464 2.83360559 5.2558966 2.99838684 5.47229057 2.36412745 6.22511283 8.60217429 4.43557381 4.46123279 12.2276683 10.8174332 3.31388158 13.236264
  1 5.15439299 5.00397071 2.68410269 4.47586472 2.99838684 1.99932446 2.23762545 2.09963611 4.49102523 4.00318276 2.76663778 8.30246798 10.9081016 3.49155158 13.236264
]))
(def rate_table (tensor @shape [7 16] @data [
  1.08709848 5.45233905 2.89649437 2.15273075 0.91033233 1.45504618 3.65521494 1.76759978 4.93289681 2.01128069 4.67300222 1.51357876 0.834242928 0.330230868 1.01442682 1.45144845
  2.14971644 10.0141619 3.17118746 5.63209967 1.15819824 1.50612992 4.99846631 2.00438989 4.19662832 1.18657549 3.84465238 2.06301673 3.608064 2.74328808 1.82563118 1.45144845
  1.37615873 12.0257113 4.7280668 4.86154559 0.892832497 1.55721366 5.31789255 2.24118001 6.60892574 2.06772731 3.01630254 2.61245469 6.38188507 5.1563453 2.27845735 1.45144845
  1.34514044 7.41841353 7.74372833 3.03552055 0.841868911 1.60829739 8.07824388 2.47797012 7.71788449 2.94887912 2.1879527 3.16189266 9.15570615 7.56940251 1.88625839 1.45144845
  1.38116968 10.3813209 9.34520792 2.40933936 1.2120538 4.17190681 6.41912578 2.89785884 6.09253605 4.21830258 3.88055434 3.67399728 9.41044481 9.12659952 2.37212887 1.45144845
  1.37441551 7.25590238 4.83910526 2.10933887 3.86225109 4.17190681 4.76000768 1.66051031 4.4671876 7.70218588 5.02765965 4.1861019 10.1553797 5.10215399 0.979550914 1.45144845
  1.86949666 8.42532373 1.88398877 7.57554478 5.15028607 4.17190681 3.10088958 3.69983045 2.84183916 4.01475207 3.03602053 4.69820651 5.99493057 5.15911912 1.71255017 1.45144845
]))
(def rise_table (tensor @shape [7 16] @data [
  0.0695456127 0.0384958252 0.0676749785 0.0203843439 0.12684487 0.00122018349 0.132558745 0.00130406023 0.00165763637 0.0289746608 0.0184062488 0.0013528647 0.0010182724 0.0010002606 0.00109631225 0.00100410242
  0.0762142452 0.00109637241 0.0129828848 0.0498900412 0.0858126627 0.00339155788 0.00956205367 0.00430260378 0.0207523275 0.00100035546 0.0168950238 0.00123733433 0.100678848 0.0176977155 0.14254496 0.00100410242
  0.0790088406 0.0531349538 0.0820330921 0.0321722543 0.109929916 0.00556293227 0.00951486297 0.00730114734 0.0313539104 0.0289983533 0.0153837987 0.00112180396 0.200339424 0.0343951704 0.014798848 0.00100410242
  0.0298983471 0.039124994 0.3 0.168818566 0.00100000005 0.00773430666 0.0539884028 0.0102996909 0.0219930999 0.0569963512 0.0138725737 0.0010062736 0.3 0.0510926253 0.00100002465 0.00100410242
  0.0232396312 0.0265231644 0.0230209186 0.149993606 0.0533820433 0.134990437 0.0546082481 0.177989248 0.0380322723 0.197096054 0.151772187 0.100670849 0.286744569 0.0323286245 0.00100246048 0.00100410242
  0.299999998 0.27603521 0.3 0.244335409 0.00100095968 0.134990437 0.0552280934 0.00100000004 0.0540714448 0.0363611593 0.115994423 0.200335425 0.3 0.0908536322 0.215561516 0.00100410242
  0.0500408308 0.0127872526 0.0661254814 0.0119347606 0.0481460162 0.134990437 0.0558479387 0.0150037623 0.0701106173 0.123165465 0.167317943 0.3 0.299999906 0.0190842677 0.156152873 0.00100410242
]))
(def direct_table (tensor @shape [7 16] @data [
  0.412603795 0.797908196 0.430388472 0.636486531 0.137189462 0.988940625 0.279780589 0.686009925 0.695642788 7.77156117e-16 2.50788279e-12 0.99916461 0.99920554 0.997734282 0.782071587 0.999393605
  0.324050094 0.999566485 6.32827124e-15 0.254197294 0.494849767 0.659297122 0.11035629 0.457344786 0.889555358 6.6780644e-05 0.176545859 0.998639759 0.685339965 0.665156188 0.168096618 0.999393605
  0.410722159 0.855955515 1.11022302e-16 0.399799448 0.415443967 0.329653619 4.08423628e-10 0.228679648 5.22754395e-10 0.227257083 0.353091718 0.998114908 0.37147439 0.332578094 1.11022302e-16 0.999393605
  0.34487002 0.373099337 0.128463176 0.812242333 0.999999984 1.01154384e-05 5.77315973e-15 1.45090176e-05 1.44328993e-15 0.454447385 0.529637577 0.997590057 0.0576088149 3.45279361e-14 0.999990283 0.999393605
  3.01980663e-14 0.250328629 0.134778474 0.69937388 0.674919062 0.118058263 3.88578059e-15 0.630465506 0.121644864 0.290821557 0.382605662 0.940015413 0.0569580498 1.11022302e-16 0.9999556 0.999393605
  0.590190281 0.361115185 0.0644379214 0.568879001 0.999903813 0.118058263 1.99840144e-15 0.317065662 0.243289727 1.11022302e-16 0.482933366 0.882440768 0.0453044553 0.0288848253 0.361011236 0.999393605
  0.250723351 3.3078873e-11 0.411968663 2.27162733e-12 0.615559806 0.118058263 1.11022302e-16 5.9552363e-13 0.364934591 0.409340623 0.106486444 0.824866124 1.96675165e-09 3.18634126e-08 0.53344722 0.999393605
]))
(def amplitude_table (tensor @shape [35 16] @data [
  0.00761500095 0.000156604378 5.28682463e-05 2.8326402e-05 1.07780996e-05 3.19715592e-06 1.07804243e-06 1.44193746e-05 6.82960251e-06 1.70896507e-05 1.30683527e-06 1.33078322e-05 8.97761071e-07 5.51882528e-07 2.32310053e-06 7.60064288e-06
  0.0156205876 0.0025540123 0.000574760042 0.000135565216 0.000252222593 3.29641553e-05 7.50064086e-06 6.9116093e-05 4.69126959e-05 7.06976678e-05 5.01454105e-06 3.43199948e-05 5.72444387e-06 9.64042699e-07 5.8528698e-06 8.23594312e-07
  0.0236893932 0.00614338917 0.00114305683 0.00183510627 0.00075118117 0.000264708555 0.000147806529 0.000171251308 0.000246862382 0.00016745421 4.64332718e-05 7.19135107e-05 2.83105822e-05 9.08517861e-06 6.75372833e-05 1.82145091e-05
  0.0713145811 0.033324995 0.0104880574 0.00881812875 0.00172348448 0.00352470171 0.0015798956 0.000901661241 0.00061150207 0.000631416776 0.000267816234 0.000226299224 0.000104608135 0.000134186628 0.000193505102 0.000121565489
  0.0780698653 0.0632942156 0.032691685 0.0097547815 0.00530290478 0.00615868846 0.00456137009 0.00152372108 0.00267161315 0.00137370473 0.00190732469 0.000583583952 0.0004400964 0.000188949266 0.000350726431 0.000381625574
  0.012531061 0.00024171661 0.000108852544 0.000117604242 6.65424843e-05 0 3.98451085e-05 0 1.44188385e-05 7.10851468e-05 0 0 0 0 0.000111683603 0
  0.0689288296 0.00903569302 0.00102494378 0.00364526691 0.000880059412 0 0.000406524772 0 0.000374941262 6.58799786e-05 0 0 0 0 0.000188063948 0
  0.0853502942 0.0275598941 0.00135759699 0.00525014049 0.000739921297 0 0.000893201057 0 0.000190114049 0.000136640356 0 0 0 0 0.00036816315 0
  0.160814183 0.0770663184 0.00291105987 0.0371973328 0.000857710217 0 0.00805221147 0 0.000997995465 0.000151516084 0 0 0 0 0.000588189778 0
  0.24286268 0.197265313 0.0249472418 0.00871160577 0.00435089334 0 0.00239368011 0 0.000780670427 0.000412973786 0 0 0 0 0.00140805577 0
  0.0320378348 0.00105855311 4.88297644e-05 0.0072769778 0.000123292448 0 7.65216765e-06 0 1.92373615e-06 0 0 0 0 0 3.61056331e-05 0
  0.0851262706 0.00938501462 0.000234562209 0.0465149058 0.000965778479 0 5.17303334e-05 0 1.40216338e-05 0 0 0 0 0 7.41302587e-05 0
  0.098525291 0.0148628666 0.000446842825 0.055309725 0.00150749031 0 0.000139361297 0 0.000116099356 0 0 0 0 0 0.000198112207 0
  0.11349259 0.0411927725 0.00160156486 0.185011046 0.00154293827 0 0.000514769605 0 0.000133992272 0 0 0 0 0 0.0012834332 0
  0.155978609 0.111456082 0.00252688336 0.244642908 0.00228625586 0 0.000974833412 0 0.000611292506 0 0 0 0 0 0.00478062144 0
  0.00697329459 0.000515468456 4.97107653e-06 0.000286025517 1.18470749e-05 2.20732918e-06 2.82166594e-06 8.79542678e-06 9.02637402e-06 0.000120729622 1.56899491e-06 1.2092981e-06 7.23664707e-06 4.00344197e-06 1.34810708e-05 0
  0.0202482425 0.00543385015 0.000485662452 0.00402561562 0.000208702506 4.32599495e-05 2.34535904e-05 7.36025901e-05 6.65323556e-05 0.00120594533 9.75629539e-05 5.42294397e-05 0.00268133622 9.17570207e-05 6.3862925e-05 0
  0.0318530092 0.0124288057 0.00104672976 0.00770322472 0.000680960245 9.86855242e-05 0.000314499635 0.000199780782 0.000174290492 0.00328202233 0.00032387026 6.83668163e-05 0.00173127101 0.000299717078 8.71689401e-05 0
  0.0992369196 0.0361198083 0.00286435792 0.0428350891 0.000432437683 0.000382560923 0.000337279646 0.000700303412 0.000733195317 0.00868480443 0.00109960991 0.000148303818 0.0132189375 0.000692257159 0.000399660754 0
  0.120724098 0.0334844343 0.000957693942 0.101424435 0.000567356585 0.000848579809 0.000952591587 0.000656542516 0.000624803919 0.0194421373 0.00135465441 0.000234361291 0.00361168568 0.00135799441 0.00343580426 0
  0.0247273267 0.000425559768 3.37430835e-05 0.000808420405 9.89671603e-05 1.77569468e-05 0 6.51022568e-05 0 6.10734983e-05 3.67638346e-06 0 1.95326016e-05 2.17814154e-05 6.13141419e-06 0
  0.0512314902 0.00290313718 0.000225458946 0.00582025225 0.000654892962 0.00012052944 0 0.000274570742 0 0.000576555181 3.60155461e-05 0 0.000456283257 9.92649533e-05 0.000239656039 0
  0.0729034106 0.0107346044 0.000689193805 0.0093798469 0.000477925204 0.000353652775 0 0.000548877031 0 0.000798737149 7.80810099e-05 0 0.0017362953 0.0002660132 5.65655965e-05 0
  0.0996556761 0.0115931946 0.000838852673 0.0353015134 0.000990997465 0.000762714019 0 0.000934714077 0 0.00224201146 0.000376828303 0 0.0036250716 0.000475852079 0.000442645499 0
  0.131289262 0.0243035274 0.00276202199 0.0673913456 0.000414011296 0.00137195955 0 0.00161805732 0 0.00692603051 0.000933719539 0 0.016225789 0.00327669514 0.00323245877 0
  0.0134315956 0.000345652793 8.41197464e-06 2.89764252e-05 4.49299761e-06 0 0 3.8751046e-06 0 5.76003782e-05 2.16046268e-06 0 0.000237547053 1.44024623e-06 2.22755325e-05 0
  0.0588630247 0.00373685112 9.01929318e-05 0.00578358787 2.69311315e-05 0 0 4.82663777e-05 0 8.84875524e-05 3.21291104e-05 0 0.00069154338 6.06958432e-05 0.000555751155 0
  0.0979871475 0.0074354693 0.000140075866 0.00419625673 0.000118599588 0 0 4.8302369e-05 0 0.000353898936 7.52278735e-05 0 0.00197426016 0.000384032348 0.000613328207 0
  0.232457724 0.0304872877 0.00186911542 0.0249201364 0.000871398351 0 0 0.000698656216 0 0.00414638483 0.00105009467 0 0.0139483302 0.00102396199 0.00368688968 0
  0.383111126 0.0528774688 0.00103222962 0.0349853535 0.00165054465 0 0 0.000346405728 0 0.00115344407 0.00035159927 0 0.00633303585 0.000761771439 0.0105412347 0
  0.0111657653 0.000463128325 0.000125141747 0.000229928381 0.000157389763 0 0.000168448237 0.000133098029 3.68683553e-05 4.3883586e-05 0.00021094896 0.000151524437 0.00108666938 6.42920566e-06 6.63032576e-05 0
  0.031212391 0.00272917169 4.3639344e-05 0.00156756099 0.00048469933 0 0.000385950118 4.26221622e-05 0.000364672025 0.000381033107 6.90900971e-05 2.35791212e-05 0.000431610225 2.46818109e-05 0.000333053756 0
  0.0504915423 0.0109905344 7.87991537e-05 0.00290379571 0.00176684963 0 0.00101376887 0.000135569523 0.00875255755 0.00200013398 0.000159450654 8.39152155e-05 0.00101311403 8.59140104e-05 0.00104050685 0
  0.101265078 0.0226479591 0.000219896314 0.0153773182 0.00293460865 0 0.0023550849 0.000278278976 0.00118869839 0.00681419361 0.000690644051 0.000266180168 0.00291491712 0.000218128807 0.00216837942 0
  0.145852621 0.0331396153 0.000818178619 0.0169126811 0.0124417175 0 0.00388790856 0.00117280295 0.00291406823 0.0127014339 0.00147749575 0.000770970168 0.00707300217 0.000558308368 0.00473547918 0
]))
(def tuning_table (tensor @shape [7] @data [
  26.1189166 46.0015696 0.984635051 -33.042517 2.3494955 2.66209503 -23.8096641
]))
(def mode_pan_table (tensor @shape [16] @data [
  0 0.472843206 -0.697319729 0.555520526 -0.121927366 -0.375709636 0.676000552 -0.621213901 0.240127043 0.267089484 -0.63401399 0.667914885 -0.150303598 -0.694195687 0.45111289 0.699252824
]))
;; END GENERATED CALIBRATION

(defmacro saron-smooth (target ms)
  (make-history previous)
  (make-history ready)
  (def pole (exp (/ -1 (* 0.001 ms samplerate))))
  (def value (gswitch (read-history ready) (mix target (read-history previous) pole) target))
  (write-history previous value)
  (write-history ready 1)
  value)

(defmacro saron-pole (signal pole)
  (make-history previous)
  (def value (mix signal (read-history previous) pole))
  (write-history previous value)
  value)

;; Coefficients latch on every strike as well as each periodic control tick.
;; A periodic-only hop would miss the beginning of an off-grid force pulse.
;; These are frame-rate latches, so note events can update them immediately.
(def control_tick (eq (accum 1 0 0 16) 0))
(defmacro saron-hold (value tick) (event-hold value tick))
(defmacro saron-audio (value tick) (latch value tick))

;; Native references use D5, Eb5, F5, Ab5, A5, Bb5 and C6. Intervening keys
;; interpolate continuously; outside the observed register the end bar scales
;; with pitch. Tuning amount blends equal temperament into this ensemble.
(defmacro saron-row (note)
  (+ (clip (- note 74) 0 1)
     (clip (/ (- note 75) 2) 0 1)
     (clip (/ (- note 77) 3) 0 1)
     (clip (- note 80) 0 1)
     (clip (- note 81) 0 1)
     (clip (/ (- note 82) 2) 0 1)))

(make-history last_gate)
(def held (gt gate 0.5))
(def onset (max (gt trigger 0.5) (* held (lte (read-history last_gate) 0.5))))
(def update_tick (max control_tick onset))
(write-history last_gate held)
(def hit_velocity (latch (clip velocity 0 1) onset))
(def strength (mix 0.6 hit_velocity (clip dynamics 0 1)))
(def velocity_row (saron-hold (clip (- (* strength 5) 1) 0 4) update_tick))
(def velocity_gain (/ hit_velocity (max 0.2 strength)))
(def key_note (+ 69 (* 12 (/ (log (/ (clip pitch 32.703196 8372.018) 440)) (log 2)))))
(def row (saron-hold (saron-row (+ key_note (saron-smooth (clip (mod register) -24 24) 8))) update_tick))
(def row_low (floor row))
(def row_high (min 6 (+ row_low 1)))
(def row_mix (- row row_low))
;; Row coordinates are already clamped. Share integer gather indices across
;; fields instead of repeating wrapped fractional lookups inside each mode.
(def mode_indices (iota mode_count))
(def low_indices (+ mode_indices (* row_low mode_count)))
(def high_indices (+ mode_indices (* row_high mode_count)))
(def velocity_low (floor velocity_row))
(def velocity_high (min (- 5 1) (+ velocity_low 1)))
(def velocity_mix (- velocity_row velocity_low))
(def a00 (+ mode_indices (* (+ (* row_low 5) velocity_low) mode_count)))
(def a01 (+ mode_indices (* (+ (* row_low 5) velocity_high) mode_count)))
(def a10 (+ mode_indices (* (+ (* row_high 5) velocity_low) mode_count)))
(def a11 (+ mode_indices (* (+ (* row_high 5) velocity_high) mode_count)))
(defmacro saron-row-mix (table lower upper fraction)
  (mix (gather table lower) (gather table upper) fraction))

(def base_hz (* (clip pitch 32.703196 8372.018)
  (pow 2 (/ (+ (clip (mod tune) -100 100)
    (* (clip (mod amount) 0 1) (peek tuning_table (saron-row key_note)))) 1200))))
(def ratio (saron-row-mix ratio_table low_indices high_indices row_mix))
(def harmonic_ratio (max 1 (floor (+ ratio 0.5))))
(def live_ratio (max 1 (mix harmonic_ratio ratio (saron-hold (saron-smooth (clip (mod inharmonicity) 0 1.8) 8) update_tick))))
(def frequencies (* (saron-hold base_hz update_tick) live_ratio))
(def band (clip (/ (- (* samplerate 0.47) frequencies) (* samplerate 0.07)) 0 1))
(def amplitude (mix
  (saron-row-mix amplitude_table a00 a01 velocity_mix)
  (saron-row-mix amplitude_table a10 a11 velocity_mix) row_mix))

;; A normalized two-pole contact force; measured modal residues are deconvolved
;; by its reference response. Hardness/contact alter the force, not the decay.
(def reference_pole (exp (/ -1 (* 0.00006 samplerate))))
(def contact_tau (* 0.00006 (clip (mod contact) 0.3 3)
  (pow 2 (* 3 (- 0.5 (clip (mod hardness) 0 1))))))
(def contact_pole (exp (/ -1 (* (saron-hold contact_tau update_tick) samplerate))))
(def force_pole (saron-audio contact_pole update_tick))
(def force1 (saron-pole (* onset velocity_gain) force_pole))
(def force (saron-pole force1 force_pole))
(def omega (* twopi (/ (min frequencies (* 0.48 samplerate)) samplerate)))
(def rotation_cos (cos omega))
(def rotation_sin (sin omega))
(def inverse_contact (/ (+ (* (- 1 reference_pole) (- 1 reference_pole))
  (* 2 reference_pole (- 1 rotation_cos))) (* (- 1 reference_pole) (- 1 reference_pole))))
;; A finite mallet footprint suppresses shorter spatial wavelengths. Its width
;; is a timbral control: spatial mode shapes cannot be recovered from one mic.
(def footprint (exp (* -0.8 (- ratio 1) (saron-hold (clip (mod spread) 0 1) update_tick))))
(def color_weight (pow ratio (saron-hold (clip (mod color) -1 1) update_tick)))
(def weight (* amplitude inverse_contact band footprint color_weight))
(def decay_scale (saron-hold (saron-smooth (clip (mod decay) 0.15 4) 8) update_tick))
(def loss_scale (pow ratio (* 0.5 (- (saron-hold (saron-smooth (clip (mod loss) 0.25 4) 8) update_tick) 1))))
(def hand_loss (* 100 (pow (saron-smooth (clip (mod touch) 0 1) 4) 2)))
(def release_loss (* (- 1 held) (pow (- 1 (clip (mod lift) 0 1)) 2)
  (/ 6.907755 (clip (mod release_s) 0.02 12))))
(def rate (+ (/ (* (saron-row-mix rate_table low_indices high_indices row_mix) loss_scale) decay_scale)
  (* (saron-hold (+ hand_loss release_loss) update_tick) (sqrt ratio))))
(def rise (max 0.00005 (* (saron-row-mix rise_table low_indices high_indices row_mix)
  (saron-hold (saron-smooth (clip (mod bloom) 0 2) 8) update_tick))))
(def direct (saron-row-mix direct_table low_indices high_indices row_mix))

;; A stable modal bar followed by a passive radiation pole at each mode.
;; Its impulse envelope is exp(-rate*t) * (1 - (1-direct)*exp(-t/rise)).
;; This is a causal recurrence, not a recorded or scheduled amplitude envelope.
;; Both complex rotations remain contractive during pitch/damping automation.
(defmacro saron-bar (rotation_cos rotation_sin rate rise direct weight force tick)
  (make-tensor-history bar_r @shape [16])
  (make-tensor-history bar_i @shape [16])
  (make-tensor-history radiation_r @shape [16])
  (make-tensor-history radiation_i @shape [16])
  (def x (read-tensor-history bar_r))
  (def y (read-tensor-history bar_i))
  (def xr (read-tensor-history radiation_r))
  (def yr (read-tensor-history radiation_i))
  (def radius (exp (/ (- rate) samplerate)))
  (def c (saron-audio (* radius rotation_cos) tick))
  (def s (saron-audio (* radius rotation_sin) tick))
  (def coupling (saron-audio (exp (/ -1 (* rise samplerate))) tick))
  (def next_r (+ (- (* c x) (* s y)) (* force (saron-audio weight tick))))
  (def next_i (+ (* s x) (* c y)))
  (def radiated_r (+ (* coupling (- (* c xr) (* s yr))) (* (- 1 coupling) next_r)))
  (def radiated_i (+ (* coupling (+ (* s xr) (* c yr))) (* (- 1 coupling) next_i)))
  (write-tensor-history bar_r next_r)
  (write-tensor-history bar_i next_i)
  (write-tensor-history radiation_r radiated_r)
  (write-tensor-history radiation_i radiated_i)
  (mix radiated_i next_i (saron-audio direct tick)))

(def modes (saron-bar rotation_cos rotation_sin rate rise direct weight force update_tick))
(def stereo (saron-hold (saron-smooth (clip (mod width) 0 1) 8) update_tick))
(def left (sum (* modes (saron-audio (sqrt (+ 1 (* mode_pan_table stereo))) update_tick))))
(def right (sum (* modes (saron-audio (sqrt (- 1 (* mode_pan_table stereo))) update_tick))))

(def drive_v (saron-smooth (clip (mod drive) 0 1) 8))
(def cutoff (min (saron-smooth (clip (mod tone_hz) 1000 20000) 8) (* samplerate 0.43)))
(def output_gain (saron-smooth (clip (mod gain) 0 4) 8))
(defmacro saron-output (signal drive_v cutoff gain_v)
  (def driven (mix signal (/ (tanh (* signal (+ 1 (* drive_v 8)))) (+ 1 (* drive_v 2))) drive_v))
  (def filtered (svf driven cutoff 0.707 0))
  (* filtered gain_v))
(out (saron-output left drive_v cutoff output_gain) 1 @name left)
(out (saron-output right drive_v cutoff output_gain) 2 @name right)
